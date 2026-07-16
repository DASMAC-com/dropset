//! The Coinbase EURC/USDC reference feed process: resume from the saved cursor,
//! backfill then poll the Coinbase candles endpoint, and persist closed buckets
//! into `cex_prices` through the framework store sink (docs/fx-survey.md §4).
//! Long-lived; resumes from its cursor on restart.

use dropset_feeds::{
    connect, run, CursorStore, HttpClient, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_fx_survey::{
    coinbase::{Candle, CoinbaseCandles},
    config::Config,
    store::CexWriter,
};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let pool = connect(&cfg.database_url).await?;
    let feed = cfg.feed_name();

    // Bridge the store's saved position into the source at startup: the store
    // sink owns the cursor, the source computes its window from it.
    let resume = PgCursorStore::new(pool.clone()).load(&feed).await?;
    let source = CoinbaseCandles::resume(
        HttpClient::new(&cfg.coinbase_base_url)?,
        feed.clone(),
        cfg.product_id.clone(),
        cfg.granularity_secs,
        cfg.max_buckets_per_request,
        resume,
        cfg.backfill_start_secs,
    )?;
    tracing::info!(
        %feed,
        product = %cfg.product_id,
        granularity = cfg.granularity_secs,
        "coinbase feed starting"
    );

    let writer = CexWriter::new("coinbase", &cfg.product_id, cfg.granularity_secs);
    let sink = StoreSink::new(pool, feed, writer);
    let sinks: Vec<Box<dyn Sink<Candle>>> = vec![Box::new(sink)];
    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    run(source, sinks, run_cfg).await
}
