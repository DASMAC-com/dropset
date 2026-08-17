//! The OANDA FX anchor feed process: resume from the saved cursor, backfill
//! then poll the v20 candles endpoint, and persist closed buckets into
//! `cex_prices` (docs/data-feeds.md §9). Long-lived; resumes on restart.
//!
//! Thin on purpose — configuration, then source → sink wiring. The polling,
//! paging, and decoding are the shared `dropset_feeds::venues` adapter's.
//!
//! Poll budget: v20 documents 100 requests/second on a persistent connection,
//! so a 60-second tick has room to spare and the cadence is chosen for
//! freshness rather than to dodge a limit.

use dropset_feeds::{
    connect, run,
    venues::{Candle, OandaCandles},
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{oanda_instrument, secret, FxConfig, FxDefaults},
    store::CexWriter,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
const SOURCE: &str = "oanda";

const DEFAULTS: FxDefaults = FxDefaults {
    base_url: "https://api-fxpractice.oanda.com",
    granularity_secs: 60,
    poll_interval_secs: 60,
    max_buckets_per_request: 5_000,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = FxConfig::from_env(&DEFAULTS)?;
    let api_key = secret("OANDA_API_KEY")?;
    let instrument = oanda_instrument(&cfg.product_id)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let feed = cfg.feed_name(SOURCE);

    let resume = PgCursorStore::new(pool.clone()).load(&feed).await?;
    let source = OandaCandles::resume(
        &cfg.base_url,
        &api_key,
        feed.clone(),
        &instrument,
        cfg.granularity_secs,
        cfg.max_buckets_per_request,
        resume,
        cfg.backfill_start_secs,
    )?;
    tracing::info!(
        %feed,
        product = %cfg.product_id,
        %instrument,
        granularity = cfg.granularity_secs,
        "oanda feed starting"
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
