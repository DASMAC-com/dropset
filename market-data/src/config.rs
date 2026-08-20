//! Environment-driven configuration. `DATABASE_URL` is required (it decides
//! local Postgres vs. Aurora, docs/data-feeds.md §1); everything else has a
//! default tuned for the gate's Coinbase EURC/USDC feed, so the same binary
//! runs unchanged against localnet Postgres and, post-gate, Fargate + Aurora.

use crate::roster::{roster_from_env, RosterEntry};
use dropset_feeds::{now_secs, venues::coinbase::MAX_CANDLES_PER_REQUEST};

/// Default backfill depth — long enough to span the weekend and macro-event
/// regimes with enough repeats to matter (docs/data-feeds.md §11).
const DEFAULT_BACKFILL_DAYS: u64 = 60;

/// Seconds between polls once the feed has caught up.
///
/// **Deliberately far below the bucket width.** A 60-second poll on 60-second
/// candles means the newest closed bucket is discovered up to a full minute
/// after it closes, which is what made the collected series look like it
/// updated once a minute *and lagged*. Polling at a quarter of the bucket width
/// does not change the row rate — 60s is the finest bucket the endpoint offers,
/// so it is still one row per minute per pair — but the newest closed bucket
/// lands ~45s sooner. Four requests a minute per pair is nowhere near the
/// venue's budget; the cadence table in docs/data-feeds.md §10 records where
/// each venue's ceiling actually is.
const DEFAULT_POLL_INTERVAL_SECS: u64 = 15;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub coinbase_base_url: String,
    /// Every product this collector polls. One process serves a venue rather
    /// than a pair (see [`crate::roster`]).
    pub products: Vec<RosterEntry>,
    pub granularity_secs: i64,
    /// Epoch second the backfill starts from. Only used the first time a feed
    /// runs; afterwards the saved cursor wins.
    pub backfill_start_secs: i64,
    /// Buckets per Coinbase request (≤ [`MAX_CANDLES_PER_REQUEST`]).
    pub max_buckets_per_request: usize,
    /// Sleep between polls once the feed has caught up to the present.
    pub poll_interval_secs: u64,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let database_url = std::env::var("DATABASE_URL")
            .map_err(|_| anyhow::anyhow!("DATABASE_URL is required"))?;
        let granularity_secs = env_or("GRANULARITY_SECS", "60").parse().unwrap_or(60);
        let backfill_days = env_or("BACKFILL_DAYS", &DEFAULT_BACKFILL_DAYS.to_string())
            .parse()
            .unwrap_or(DEFAULT_BACKFILL_DAYS);
        // An explicit epoch overrides the day-count default; otherwise start
        // `backfill_days` before now, aligned down to a bucket boundary.
        let backfill_start_secs = match std::env::var("BACKFILL_START_SECS") {
            Ok(v) => v
                .parse()
                .unwrap_or_else(|_| default_start(granularity_secs, backfill_days)),
            Err(_) => default_start(granularity_secs, backfill_days),
        };
        let max_buckets_per_request = env_or(
            "MAX_BUCKETS_PER_REQUEST",
            &MAX_CANDLES_PER_REQUEST.to_string(),
        )
        .parse()
        .unwrap_or(MAX_CANDLES_PER_REQUEST)
        .min(MAX_CANDLES_PER_REQUEST);
        Ok(Self {
            database_url,
            coinbase_base_url: env_or("COINBASE_BASE_URL", "https://api.exchange.coinbase.com"),
            products: roster_from_env("EURC-USDC")?,
            granularity_secs,
            backfill_start_secs,
            max_buckets_per_request,
            poll_interval_secs: env_or(
                "POLL_INTERVAL_SECS",
                &DEFAULT_POLL_INTERVAL_SECS.to_string(),
            )
            .parse()
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECS),
        })
    }

    /// The framework feed identifier (cursor key, log/metric label) — stable
    /// across restarts, e.g. `cex:coinbase:EURC-USDC`.
    ///
    /// Keyed by product rather than by process, which is what lets one roster
    /// service take over from several per-pair ones without resetting a single
    /// cursor: the name a pair's cursor was saved under does not depend on how
    /// many pairs share its process.
    pub fn feed_name(&self, product_id: &str) -> String {
        format!("cex:coinbase:{product_id}")
    }
}

/// `backfill_days` before now, floored to a `granularity`-aligned bucket start.
fn default_start(granularity: i64, backfill_days: u64) -> i64 {
    let start = now_secs() - (backfill_days as i64) * 86_400;
    let granularity = granularity.max(1);
    start - start.rem_euclid(granularity)
}
