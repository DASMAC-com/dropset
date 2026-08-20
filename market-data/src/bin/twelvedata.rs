//! The Twelve Data FX corroboration collector: for every pair on its roster,
//! resume from that pair's saved cursor, backfill then poll the time-series
//! endpoint, and persist closed bars into `cex_prices` (docs/data-feeds.md §9).
//!
//! **Poll budget: 800 credits/day on the free tier, account-wide.** The
//! per-pair default of five minutes is 288 requests/day for one pair — already
//! most of a single pair's fair share — so a roster of any size has to widen
//! the cadence to stay inside the quota. `fx::quota_floor_secs` does that, and
//! the effective interval is logged at startup so the widening is never a
//! surprise.

use dropset_feeds::{
    connect, run,
    venues::{twelvedata, Candle, TwelveDataCandles},
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{quota_floor_secs, secret, twelvedata_symbol, FxConfig, FxDefaults},
    store::CexWriter,
    supervise::run_all,
    ticks::by_venue_symbol,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
const SOURCE: &str = "twelvedata";

/// The share of the free tier's 800 daily credits this collector will spend,
/// leaving the rest for restarts (each of which re-polls every pair) and for
/// anything else using the same key.
const USABLE_DAILY_REQUESTS: u64 = 600;

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
    let api_key = secret(twelvedata::SECRET_NAME)?;
    let symbols = by_venue_symbol(&cfg.products, twelvedata_symbol)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let cursors = PgCursorStore::new(pool.clone());

    // One transport for the process, cloned per feed: on a metered tier a
    // client per pair would hand each pair the whole account's budget. See
    // `TwelveDataCandles::client`.
    let http = TwelveDataCandles::client(&cfg.base_url, &api_key)?;

    let poll_secs = quota_floor_secs(
        cfg.poll_interval_secs,
        symbols.len(),
        USABLE_DAILY_REQUESTS,
    );
    if poll_secs != cfg.poll_interval_secs {
        tracing::info!(
            configured = cfg.poll_interval_secs,
            effective = poll_secs,
            products = symbols.len(),
            "widened the poll interval so the roster fits the venue's daily quota"
        );
    }
    tracing::info!(
        products = %symbols.iter().map(|(_, p)| p.as_str()).collect::<Vec<_>>().join(","),
        granularity = cfg.granularity_secs,
        poll_secs,
        "twelvedata collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(poll_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(symbols.len());
    for (symbol, product_id) in symbols {
        let feed = cfg.feed_name(SOURCE, &product_id);
        let resume = cursors.load(&feed).await?;
        let source = TwelveDataCandles::resume(
            http.clone(),
            feed.clone(),
            &symbol,
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
