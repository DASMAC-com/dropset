// cspell:word AUDUSD
//! FX collector configuration, credential resolution, and the canonical ↔
//! venue symbol mapping the three FX feeds share.
//!
//! **The stored `product_id` is canonical and the venue's symbol is derived
//! from it**, never the other way round. The three vendors spell the same pair
//! three different ways — OANDA `AUD_USD`, Twelve Data `AUD/USD`, Alpha Vantage
//! as two separate `from_symbol` / `to_symbol` parameters — so writing whatever
//! each vendor happens to call it would land one pair under three keys and make
//! a cross-source comparison impossible. Everything here writes `AUD-USD`,
//! matching the hyphenated style the Coinbase rows already use, and each
//! adapter is handed the spelling it wants at construction.

use anyhow::{anyhow, Context, Result};
use dropset_feeds::{now_secs, secrets::SecretProvider};

/// Default backfill depth, matching the Coinbase collector's — deep enough to
/// span the weekend and macro-event regimes (docs/data-feeds.md §11). The FX
/// venues serve far more than this (OANDA reaches back years, Alpha Vantage to
/// 2007), so this is a cost choice rather than a limit.
const DEFAULT_BACKFILL_DAYS: u64 = 60;

/// The per-venue starting points a binary supplies, since a poll budget and a
/// request cap are properties of the venue rather than of the deployment.
pub struct FxDefaults {
    /// The venue's API root.
    pub base_url: &'static str,
    /// Bucket width. Only Alpha Vantage is pinned (daily); the others default
    /// to minute bars and can be widened by environment.
    pub granularity_secs: i64,
    /// Seconds between polls once caught up — sized to the venue's documented
    /// free-tier budget with headroom (docs/data-feeds.md §10).
    pub poll_interval_secs: u64,
    /// Buckets per request, clamped to the venue's own cap by its adapter.
    pub max_buckets_per_request: usize,
}

/// One FX collector's configuration.
pub struct FxConfig {
    pub database_url: String,
    pub base_url: String,
    /// The canonical stored symbol, e.g. `AUD-USD`.
    pub product_id: String,
    pub granularity_secs: i64,
    /// Epoch second the backfill starts from. Only used the first time a feed
    /// runs; afterwards the saved cursor wins.
    pub backfill_start_secs: i64,
    pub max_buckets_per_request: usize,
    pub poll_interval_secs: u64,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl FxConfig {
    /// Read the collector's configuration, starting from its venue's defaults.
    pub fn from_env(defaults: &FxDefaults) -> Result<Self> {
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| anyhow!("DATABASE_URL is required"))?;
        let granularity_secs = env_or("GRANULARITY_SECS", &defaults.granularity_secs.to_string())
            .parse()
            .unwrap_or(defaults.granularity_secs);
        let backfill_days = env_or("BACKFILL_DAYS", &DEFAULT_BACKFILL_DAYS.to_string())
            .parse()
            .unwrap_or(DEFAULT_BACKFILL_DAYS);
        let backfill_start_secs = match std::env::var("BACKFILL_START_SECS") {
            Ok(v) => v
                .parse()
                .unwrap_or_else(|_| default_start(granularity_secs, backfill_days)),
            Err(_) => default_start(granularity_secs, backfill_days),
        };
        Ok(Self {
            database_url,
            base_url: env_or("FX_BASE_URL", defaults.base_url),
            product_id: env_or("PRODUCT_ID", "AUD-USD"),
            granularity_secs,
            backfill_start_secs,
            max_buckets_per_request: env_or(
                "MAX_BUCKETS_PER_REQUEST",
                &defaults.max_buckets_per_request.to_string(),
            )
            .parse()
            .unwrap_or(defaults.max_buckets_per_request),
            poll_interval_secs: env_or(
                "POLL_INTERVAL_SECS",
                &defaults.poll_interval_secs.to_string(),
            )
            .parse()
            .unwrap_or(defaults.poll_interval_secs),
        })
    }

    /// The framework feed identifier (cursor key, log label) — stable across
    /// restarts, e.g. `fx:oanda:AUD-USD`. The `source` here is the same string
    /// written to `cex_prices.source`, so a cursor and its rows stay legible
    /// together.
    pub fn feed_name(&self, source: &str) -> String {
        format!("fx:{source}:{}", self.product_id)
    }
}

/// `backfill_days` before now, floored to a `granularity`-aligned bucket start.
fn default_start(granularity: i64, backfill_days: u64) -> i64 {
    let start = now_secs() - (backfill_days as i64) * 86_400;
    let granularity = granularity.max(1);
    start - start.rem_euclid(granularity)
}

/// Resolve one credential by its canonical `<provider>/<secret>` name.
///
/// **This is the single place a collector reads a secret**, and it is now a
/// thin call into the shared provider ([`dropset_feeds::secrets`]) rather than
/// an `env::var`: the environment is consulted first, then the local 1Password
/// enclave when `DROPSET_OP_VAULT` names a vault, and the AWS Secrets Manager
/// backend slots into the same chain for hosted runs. No adapter reads the
/// environment at all — a venue takes its key as an argument
/// (docs/data-feeds.md §4).
///
/// Called **once per binary, at startup**. Nothing here is on a poll path, so
/// the 1Password subprocess is paid once per process or not at all.
pub fn secret(name: &str) -> Result<String> {
    SecretProvider::from_env()
        .resolve(name)
        .with_context(|| format!("{name} is required (the API credential for this feed)"))
}

/// Split a canonical `BASE-QUOTE` symbol into its two ISO-4217 legs.
///
/// Both legs must be exactly three ASCII letters. The charset check is not
/// decoration: a derived leg is interpolated into the OANDA request **path**
/// (`/v3/instruments/{instrument}/candles`), so a length check alone would
/// admit a three-character leg carrying a `/` and let a malformed
/// `PRODUCT_ID` reshape the request path. The value is operator config rather
/// than untrusted input, so this is a guard rail, not a boundary — but it
/// costs one predicate.
pub fn split_canonical(product_id: &str) -> Result<(&str, &str)> {
    let (base, quote) = product_id
        .split_once('-')
        .ok_or_else(|| anyhow!("{product_id:?} is not a canonical BASE-QUOTE symbol"))?;
    let is_code = |leg: &str| leg.len() == 3 && leg.bytes().all(|b| b.is_ascii_alphabetic());
    if !is_code(base) || !is_code(quote) {
        return Err(anyhow!(
            "{product_id:?} is not a canonical BASE-QUOTE symbol: both legs must \
             be three-letter ISO-4217 codes"
        ));
    }
    Ok((base, quote))
}

/// `AUD-USD` → `AUD_USD`, the v20 instrument spelling.
pub fn oanda_instrument(product_id: &str) -> Result<String> {
    let (base, quote) = split_canonical(product_id)?;
    Ok(format!("{base}_{quote}"))
}

/// `AUD-USD` → `AUD/USD`, the Twelve Data symbol spelling.
pub fn twelvedata_symbol(product_id: &str) -> Result<String> {
    let (base, quote) = split_canonical(product_id)?;
    Ok(format!("{base}/{quote}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_feeds::secrets::{env_var, validate_name};

    #[test]
    fn each_venue_gets_its_own_spelling_of_one_canonical_pair() {
        assert_eq!(oanda_instrument("AUD-USD").unwrap(), "AUD_USD");
        assert_eq!(twelvedata_symbol("AUD-USD").unwrap(), "AUD/USD");
        assert_eq!(split_canonical("AUD-USD").unwrap(), ("AUD", "USD"));
    }

    #[test]
    fn a_venue_native_symbol_is_rejected_rather_than_silently_stored() {
        // The failure this guards: passing a vendor's own spelling straight
        // through as the canonical product_id, which would write the same pair
        // under a second key and break every cross-source comparison.
        assert!(split_canonical("AUD_USD").is_err());
        assert!(split_canonical("AUD/USD").is_err());
        assert!(split_canonical("AUDUSD").is_err());
    }

    #[test]
    fn a_crypto_product_is_rejected_as_an_fx_pair() {
        // AUDD-USDC parses as BASE-QUOTE but its legs are not currency codes,
        // and an FX venue would 404 on it.
        assert!(split_canonical("AUDD-USDC").is_err());
    }

    #[test]
    fn a_leg_that_is_the_right_length_but_not_letters_is_rejected() {
        // The motivating case: a derived leg is interpolated into OANDA's
        // request path, so a three-character leg carrying a separator would
        // reshape the path rather than name an instrument.
        assert!(split_canonical("a/b-c/d").is_err());
        assert!(split_canonical("../-USD").is_err());
        assert!(split_canonical("AU1-USD").is_err());
        // The legitimate shape still passes, in either case.
        assert!(split_canonical("aud-usd").is_ok());
    }

    #[test]
    fn a_credential_resolves_by_its_canonical_name() {
        // The collectors name a secret canonically and never name a variable;
        // the provider derives the environment spelling. This is the seam that
        // keeps the same name valid against the 1Password enclave and, later,
        // AWS Secrets Manager.
        //
        // Scoped to a name nothing else uses, since the process environment is
        // shared across tests in a binary.
        let name = "fx-probe/api-key";
        std::env::set_var(env_var(name), "a-real-key");
        assert_eq!(secret(name).unwrap(), "a-real-key");
        std::env::remove_var(env_var(name));

        // An unresolvable credential names the feed it belongs to, on top of
        // the per-store spellings the provider lists.
        let err = secret(name).unwrap_err().to_string();
        assert!(err.contains("the API credential for this feed"), "{err}");
    }

    #[test]
    fn every_fx_venue_names_its_credential_canonically() {
        // A malformed constant would only surface when that collector ran, so
        // assert the roster's three names parse here instead.
        for name in [
            dropset_feeds::venues::oanda::SECRET_NAME,
            dropset_feeds::venues::twelvedata::SECRET_NAME,
            dropset_feeds::venues::alphavantage::SECRET_NAME,
        ] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn the_feed_name_pairs_a_cursor_with_the_rows_it_wrote() {
        let cfg = FxConfig {
            database_url: String::new(),
            base_url: String::new(),
            product_id: "AUD-USD".to_string(),
            granularity_secs: 60,
            backfill_start_secs: 0,
            max_buckets_per_request: 5_000,
            poll_interval_secs: 60,
        };
        assert_eq!(cfg.feed_name("oanda"), "fx:oanda:AUD-USD");
    }
}
