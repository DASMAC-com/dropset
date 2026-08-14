//! The Twelve Data FX cross-check feed process: resume from the saved cursor,
//! backfill then poll the time-series endpoint, and persist closed buckets into
//! `cex_prices` (docs/data-feeds.md §9). Long-lived; resumes on restart.
//!
//! **Poll budget: 800 API credits per day and 8 requests per minute.** A
//! 60-second tick would spend 1440 credits and exhaust the account before the
//! day was out, so the default is 300 seconds — 288 requests a day, roughly a
//! third of the budget, leaving room for restarts and a backfill running
//! alongside (docs/data-feeds.md §10). That does **not** coarsen the bars: one
//! request returns a window of them, so a five-minute tick still yields a
//! continuous 60-second series, just delivered in less frequent batches.

use dropset_feeds::{
    connect, run,
    venues::{Candle, TwelveDataCandles},
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{secret, twelvedata_symbol, FxConfig, FxDefaults},
    store::CexWriter,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
const SOURCE: &str = "twelvedata";

const DEFAULTS: FxDefaults = FxDefaults {
    base_url: "https://api.twelvedata.com",
    granularity_secs: 60,
    // See the module note: sized to 800 credits/day with headroom.
    poll_interval_secs: 300,
    max_buckets_per_request: 5_000,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = FxConfig::from_env(&DEFAULTS)?;
    let api_key = secret("TWELVE_DATA_API_KEY")?;
    let symbol = twelvedata_symbol(&cfg.product_id)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let feed = cfg.feed_name(SOURCE);

    let resume = PgCursorStore::new(pool.clone()).load(&feed).await?;
    let source = TwelveDataCandles::resume(
        &cfg.base_url,
        &api_key,
        feed.clone(),
        &symbol,
        cfg.granularity_secs,
        cfg.max_buckets_per_request,
        resume,
        cfg.backfill_start_secs,
    )?;
    tracing::info!(
        %feed,
        product = %cfg.product_id,
        %symbol,
        granularity = cfg.granularity_secs,
        poll_secs = cfg.poll_interval_secs,
        "twelvedata feed starting"
    );

    let writer = CexWriter::new(SOURCE, &cfg.product_id, cfg.granularity_secs);
    let sink = StoreSink::new(pool, feed, writer);
    let sinks: Vec<Box<dyn Sink<Candle>>> = vec![Box::new(sink)];
    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    run(source, sinks, run_cfg).await
}
