//! Environment-driven configuration. `DATABASE_URL` is required (it decides
//! local Postgres vs. Aurora, docs/data-feeds.md §1); everything else has a
//! default tuned for the gate's Coinbase EURC/USDC feed, so the same binary
//! runs unchanged against localnet Postgres and, post-gate, Fargate + Aurora.

use std::time::{SystemTime, UNIX_EPOCH};

/// Coinbase's per-request candle cap: a range wider than this many buckets is
/// rejected, so the backfill pages in windows no larger (docs/fx-survey.md §4).
const COINBASE_MAX_CANDLES: usize = 300;

/// Default backfill depth — long enough to span the weekend and macro-event
/// regimes with enough repeats to matter (docs/fx-survey.md §8).
const DEFAULT_BACKFILL_DAYS: u64 = 60;

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub coinbase_base_url: String,
    pub product_id: String,
    pub granularity_secs: i64,
    /// Epoch second the backfill starts from. Only used the first time a feed
    /// runs; afterwards the saved cursor wins.
    pub backfill_start_secs: i64,
    /// Buckets per Coinbase request (≤ [`COINBASE_MAX_CANDLES`]).
    pub max_buckets_per_request: usize,
    /// Sleep between polls once the feed has caught up to the present.
    pub poll_interval_secs: u64,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// Current wall-clock epoch second.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
        let max_buckets_per_request = env_or("MAX_BUCKETS_PER_REQUEST", "300")
            .parse()
            .unwrap_or(COINBASE_MAX_CANDLES)
            .min(COINBASE_MAX_CANDLES);
        Ok(Self {
            database_url,
            coinbase_base_url: env_or("COINBASE_BASE_URL", "https://api.exchange.coinbase.com"),
            product_id: env_or("PRODUCT_ID", "EURC-USDC"),
            granularity_secs,
            backfill_start_secs,
            max_buckets_per_request,
            poll_interval_secs: env_or("POLL_INTERVAL_SECS", "60").parse().unwrap_or(60),
        })
    }

    /// The framework feed identifier (cursor key, log/metric label) — stable
    /// across restarts, e.g. `cex:coinbase:EURC-USDC`.
    pub fn feed_name(&self) -> String {
        format!("cex:coinbase:{}", self.product_id)
    }
}

/// `backfill_days` before now, floored to a `granularity`-aligned bucket start.
fn default_start(granularity: i64, backfill_days: u64) -> i64 {
    let start = now_secs() - (backfill_days as i64) * 86_400;
    let granularity = granularity.max(1);
    start - start.rem_euclid(granularity)
}
