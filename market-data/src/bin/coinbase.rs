//! The Coinbase candle collector: for every product on its roster, resume from
//! that product's saved cursor, backfill then poll the Coinbase candles
//! endpoint, and persist closed buckets into `cex_prices` through the framework
//! store sink (docs/data-feeds.md §9). Long-lived; resumes from its cursors on
//! restart.
//!
//! It is thin on purpose — configuration, then source → sink wiring per
//! product. The polling, paging, and decoding are the shared
//! `dropset_feeds::venues` adapter's; all this crate adds is the row mapping
//! and the roster.

use dropset_feeds::{
    connect, run,
    venues::{Candle, CoinbaseCandles},
    CursorStore, HttpClient, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    config::Config, instruments::register as register_instruments, roster::canonical_only,
    store::CexWriter, supervise::run_all,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
///
/// Named rather than inlined at the writer, as every sibling collector already
/// does: the instruments dimension has to be registered under the same string
/// the rows are written with, and two literals that must agree are one edit
/// away from disagreeing.
const SOURCE: &str = "coinbase";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = Config::from_env()?;
    let pool = connect(&cfg.database_url).await?;
    // A collector is DB-primary: without `cex_prices` and `feed_cursors` there
    // is nothing for it to do, so assert the schema up front rather than
    // failing on the first committed batch. `dropset-migrate` owns creating
    // them (docs/data-feeds.md §8).
    dropset_db_schema::require_schema(&pool).await?;
    let cursors = PgCursorStore::new(pool.clone());

    // **One client for the process, cloned per feed.** This is not a saving,
    // it is the rate-limit invariant: an `HttpClient`'s clones share one
    // request-pacing budget, while a second `HttpClient::new` opens an
    // independent one. Since every feed here talks to the same host from the
    // same egress IP — which is what a keyless tier limits on
    // (docs/data-feeds.md §10) — building a client per product would multiply
    // the venue's budget by the roster size and quietly break the discipline
    // the transport exists to hold.
    let http = HttpClient::new(&cfg.coinbase_base_url)?;

    // Coinbase names its products the canonical way already, so nothing is
    // derived here — which is exactly why a pinned venue spelling has to be
    // rejected rather than quietly ignored.
    let products = canonical_only(&cfg.products)?;
    // Publish the roster as the instruments dimension, so a dashboard can ask
    // what kind of thing each product is without a hardcoded product list.
    register_instruments(&pool, SOURCE, &products).await?;
    tracing::info!(
        products = %products.join(","),
        granularity = cfg.granularity_secs,
        poll_secs = cfg.poll_interval_secs,
        "coinbase candle collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(products.len());
    for product_id in products {
        // Per-product cursor key, so a roster service resumes exactly where
        // the per-pair services it replaces left off.
        let feed = cfg.feed_name(&product_id);
        let resume = cursors.load(&feed).await?;
        let source = CoinbaseCandles::resume(
            http.clone(),
            feed.clone(),
            product_id.clone(),
            cfg.granularity_secs,
            cfg.max_buckets_per_request,
            resume,
            cfg.backfill_start_secs,
        )?;
        let writer = CexWriter::new(SOURCE, &product_id, cfg.granularity_secs);
        let sinks: Vec<Box<dyn Sink<Candle>>> =
            vec![Box::new(StoreSink::new(pool.clone(), feed.clone(), writer))];
        feeds.push((feed, run(source, sinks, run_cfg.clone())));
    }
    run_all(feeds).await
}
