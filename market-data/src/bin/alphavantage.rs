//! The Alpha Vantage daily FX collector: for every pair on its roster, poll
//! `FX_DAILY` and persist closed daily bars into `cex_prices`
//! (docs/data-feeds.md §9). Long-lived; resumes from its cursors on restart.
//!
//! **Poll budget: 25 requests per day for the entire account**, which is the
//! tightest of the FX feeds by a wide margin and the one a roster breaks
//! fastest. Six hours between polls is four requests a day for a single pair;
//! seven pairs at that cadence is 28, over the whole account's quota. So the
//! interval is widened to fit the roster (`fx::quota_floor_secs`) and the
//! effective value is logged.
//!
//! Widening costs nothing here, which is why this venue tolerates it: the
//! endpoint takes no window and returns the **whole published series** on every
//! call, so one poll backfills every bar missed since the last. A slower cadence
//! makes a bar land later, never absent.
//!
//! This feed is daily-only: `FX_INTRADAY` is premium-gated on the free tier, so
//! it corroborates the daily close and cannot stand in for the OANDA anchor.
//! `GRANULARITY_SECS` is therefore not honoured here — the value is fixed.

use dropset_feeds::{
    connect, run,
    venues::{
        alphavantage::{self, GRANULARITY_SECS},
        AlphaVantageDaily, Candle,
    },
    CursorStore, PgCursorStore, RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::{quota_floor_secs, secret, split_canonical, FxConfig, FxDefaults},
    store::CexWriter,
    supervise::run_all,
};
use std::time::Duration;

/// The value written to `cex_prices.source`.
const SOURCE: &str = "alphavantage";

/// The share of the free tier's 25 daily requests this collector will spend.
/// The remainder is deliberate headroom: every restart re-polls the whole
/// roster, and the same key may be shared.
const USABLE_DAILY_REQUESTS: u64 = 20;

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
    let api_key = secret(alphavantage::SECRET_NAME)?;
    // Validate every pair up front: this venue splits a canonical id into two
    // parameters, and a malformed one must fail startup rather than become a
    // series that never appears.
    let mut pairs = Vec::with_capacity(cfg.products.len());
    for entry in &cfg.products {
        let (from_symbol, to_symbol) = split_canonical(&entry.product_id)?;
        pairs.push((
            entry.product_id.clone(),
            from_symbol.to_string(),
            to_symbol.to_string(),
        ));
    }
    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    let cursors = PgCursorStore::new(pool.clone());

    // One transport for the process, cloned per feed. On a 25-request account
    // quota a client per pair would give every pair the whole budget. See
    // `AlphaVantageDaily::client`.
    let http = AlphaVantageDaily::client(&cfg.base_url, &api_key)?;

    let poll_secs = quota_floor_secs(cfg.poll_interval_secs, pairs.len(), USABLE_DAILY_REQUESTS);
    if poll_secs != cfg.poll_interval_secs {
        tracing::info!(
            configured = cfg.poll_interval_secs,
            effective = poll_secs,
            products = pairs.len(),
            "widened the poll interval so the roster fits the account's daily quota"
        );
    }
    tracing::info!(
        products = %pairs.iter().map(|(p, _, _)| p.as_str()).collect::<Vec<_>>().join(","),
        poll_secs,
        "alphavantage daily collector starting"
    );

    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(poll_secs),
        ..RunConfig::default()
    };
    let mut feeds = Vec::with_capacity(pairs.len());
    for (product_id, from_symbol, to_symbol) in pairs {
        let feed = cfg.feed_name(SOURCE, &product_id);
        let resume = cursors.load(&feed).await?;
        let source = AlphaVantageDaily::resume(
            http.clone(),
            feed.clone(),
            &from_symbol,
            &to_symbol,
            resume,
            cfg.backfill_start_secs,
        )?;
        // The granularity written to the row is the venue's fixed one, not the
        // configured value: this feed cannot serve anything but daily bars.
        let writer = CexWriter::new(SOURCE, &product_id, GRANULARITY_SECS);
        let sinks: Vec<Box<dyn Sink<Candle>>> =
            vec![Box::new(StoreSink::new(pool.clone(), feed.clone(), writer))];
        feeds.push((feed, run(source, sinks, run_cfg.clone())));
    }
    run_all(feeds).await
}
