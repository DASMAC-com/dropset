//! The Alpha Vantage daily FX feed process: poll `FX_DAILY` and persist closed
//! daily bars into `cex_prices` (docs/data-feeds.md §9). Long-lived; resumes
//! from its cursor on restart.
//!
//! **Poll budget: 25 requests per day for the entire account**, which is the
//! tightest of the three FX feeds by a wide margin. The default tick is six
//! hours — four requests a day, leaving the rest of the budget for restarts and
//! for anything else that shares the key (docs/data-feeds.md §10). A daily bar
//! only changes once a day, so a tighter cadence would buy nothing.
//!
//! This feed is daily-only: `FX_INTRADAY` is premium-gated on the free tier, so
//! it corroborates the daily close and cannot stand in for the OANDA anchor.
//! `GRANULARITY_SECS` is therefore not honoured here — the value is fixed.

use dropset_feeds::{
    connect, run,
    venues::{alphavantage::GRANULARITY_SECS, AlphaVantageDaily, Candle},
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{secret, split_canonical, FxConfig, FxDefaults},
    store::CexWriter,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
const SOURCE: &str = "alphavantage";

const DEFAULTS: FxDefaults = FxDefaults {
    base_url: "https://www.alphavantage.co",
    granularity_secs: GRANULARITY_SECS,
    // See the module note: 25 requests/day for the whole account.
    poll_interval_secs: 21_600,
    // The endpoint takes no window; it returns the whole published series.
    max_buckets_per_request: 0,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = FxConfig::from_env(&DEFAULTS)?;
    let api_key = secret("ALPHA_VANTAGE_API_KEY")?;
    let (from_symbol, to_symbol) = split_canonical(&cfg.product_id)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let feed = cfg.feed_name(SOURCE);

    let resume = PgCursorStore::new(pool.clone()).load(&feed).await?;
    let source = AlphaVantageDaily::resume(
        &cfg.base_url,
        &api_key,
        feed.clone(),
        from_symbol,
        to_symbol,
        resume,
        cfg.backfill_start_secs,
    )?;
    tracing::info!(
        %feed,
        product = %cfg.product_id,
        poll_secs = cfg.poll_interval_secs,
        "alphavantage daily feed starting"
    );

    // The granularity written to the row is the venue's fixed one, not the
    // configured value: this feed cannot serve anything but daily bars.
    let writer = CexWriter::new(SOURCE, &cfg.product_id, GRANULARITY_SECS);
    let sink = StoreSink::new(pool, feed, writer);
    let sinks: Vec<Box<dyn Sink<Candle>>> = vec![Box::new(sink)];
    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    run(source, sinks, run_cfg).await
}
