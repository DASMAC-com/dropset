//! The OANDA FX anchor collector: for every pair on its roster, resume from
//! that pair's saved cursor, backfill then poll the v20 candles endpoint, and
//! persist closed buckets into `cex_prices` (docs/data-feeds.md §9).
//! Long-lived; resumes on restart.
//!
//! Thin on purpose — configuration, then source → sink wiring per pair. The
//! polling, paging, and decoding are the shared `dropset_feeds::venues`
//! adapter's.
//!
//! Poll budget: v20 documents 100 requests/second on a persistent connection,
//! so even a large roster on a 60-second tick has room to spare and the cadence
//! is chosen for freshness rather than to dodge a limit. This is the one FX
//! venue here whose quota a roster cannot threaten.

use dropset_feeds::{
    connect, run,
    venues::{oanda, Candle, OandaCandles},
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{oanda_instrument, secret, FxConfig, FxDefaults},
    instruments::register as register_instruments,
    roster::resolve_venue,
    store::CexWriter,
    supervise::run_all,
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
    let api_key = secret(oanda::SECRET_NAME)?;
    // Canonical id → the venue's instrument spelling, resolved for the whole
    // roster before anything connects: a malformed pair must fail startup, not
    // be discovered as a silently missing series later.
    let instruments = resolve_venue(&cfg.products, oanda_instrument)?;
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    // The canonical ids, not the venue spellings beside them: the dimension
    // keys on what the rows are stored under.
    let products: Vec<String> = instruments.iter().map(|i| i.product_id.clone()).collect();
    register_instruments(&pool, &products).await?;
    let cursors = PgCursorStore::new(pool.clone());

    // One transport for the process, cloned per feed, so the roster draws on a
    // single request budget rather than one per pair. See
    // `OandaCandles::client`.
    let http = OandaCandles::client(&cfg.base_url, &api_key)?;

    // Both lists, because a pinned `CANONICAL=VENUE` override is honoured here:
    // without the resolved instruments in the log, a mis-pinned one is visible
    // nowhere at startup.
    tracing::info!(
        products = %instruments.iter().map(|p| p.product_id.as_str()).collect::<Vec<_>>().join(","),
        instruments = %instruments.iter().map(|p| p.venue_symbol.as_str()).collect::<Vec<_>>().join(","),
        granularity = cfg.granularity_secs,
        poll_secs = cfg.poll_interval_secs,
        "oanda collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(instruments.len());
    for resolved in instruments {
        let (instrument, product_id) = (resolved.venue_symbol, resolved.product_id);
        let feed = cfg.feed_name(SOURCE, &product_id);
        let resume = cursors.load(&feed).await?;
        let source = OandaCandles::resume(
            http.clone(),
            feed.clone(),
            &instrument,
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
