//! The ingest + aggregate worker, wired onto the shared ingestion framework
//! (docs/data-feeds.md §6): the framework's RPC poll source drives, its store
//! sink commits each batch and advances a resumable cursor, and the indexer's
//! aggregator folds the new legs behind it.

use dropset_feeds::{
    connect, run_with_metrics, BatchStats, CursorStore, FeedMetrics, PgCursorStore, RawTx,
    RpcPollSource, RunConfig, Sink, Source, StoreSink,
};
use dropset_indexer::{
    aggregate::AggregateSink,
    config::Config,
    store::{EventWriter, Store},
};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    // `Store::connect` asserts the shared schema is present; the framework's
    // pool is a second connection to the same database, for the sink's own
    // transactions and the `feed_cursors` row.
    let store = Store::connect(&cfg.database_url).await?;
    let pool = connect(&cfg.database_url).await?;

    let source = RpcPollSource::new(
        cfg.rpc_url.clone(),
        cfg.program_id,
        cfg.signature_batch_limit,
    );
    let feed = source.name().to_string();

    // Resume where the last run committed. Before the migration the poll
    // always started from the present, so a restart silently skipped whatever
    // landed while the worker was down; the framework cursor closes that.
    let cursors = PgCursorStore::new(pool.clone());
    let source = match cursors.load(&feed).await? {
        Some(cursor) => source.resume_from(&cursor)?,
        None => source,
    };

    // Order matters: the aggregator reads legs the store sink has committed.
    let sinks: Vec<Box<dyn Sink<RawTx>>> = vec![
        Box::new(StoreSink::new(
            pool,
            feed.clone(),
            EventWriter::new(cfg.program_id),
        )),
        Box::new(AggregateSink::new(store, cfg.signature_batch_limit as i64)),
    ];

    tracing::info!(
        program = %cfg.program_id,
        rpc = %cfg.rpc_url,
        %feed,
        "indexer starting"
    );
    let run_cfg = RunConfig {
        poll_interval: Duration::from_millis(cfg.poll_interval_ms),
        ..RunConfig::default()
    };
    run_with_metrics(source, sinks, run_cfg, IngestLog).await
}

/// Log how much each tick ingested, through the framework's observability
/// seam rather than inline in the loop. Silent while idle, so a quiet cluster
/// does not fill the log.
///
/// This reports what the runner can see — transactions in the batch, whether
/// the source is still draining a backlog, and how long the fetch took. The
/// two figures the pre-migration loop also printed now sit with the code that
/// owns them: rows written is a `StoreSink` debug line, and legs folded an
/// `AggregateSink` one.
struct IngestLog;

impl FeedMetrics for IngestLog {
    fn on_batch(&mut self, feed: &str, stats: &BatchStats) {
        if stats.records > 0 {
            tracing::info!(
                %feed,
                txs = stats.records,
                caught_up = stats.caught_up,
                fetch_ms = stats.fetch.as_millis() as u64,
                "indexed"
            );
        }
    }

    fn on_error(&mut self, feed: &str, error: &anyhow::Error) {
        tracing::warn!(%feed, %error, "poll failed; retrying");
    }
}
