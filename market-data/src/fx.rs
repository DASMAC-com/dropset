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

use crate::roster::{roster_from_env, RosterEntry};
use anyhow::{anyhow, Context, Result};
use dropset_feeds::{now_secs, secrets::SecretProvider};
use std::collections::HashMap;

/// Default backfill depth, matching the Coinbase collector's — deep enough to
/// span the weekend and macro-event regimes (docs/data-feeds.md §11). The FX
/// venues serve far more than this (OANDA reaches back years, Alpha Vantage to
/// 2007), so this is a cost choice rather than a limit.
const DEFAULT_BACKFILL_DAYS: u64 = 60;

/// The roster an FX collector polls when `PRODUCT_IDS` is unset.
///
/// **Shared by all three FX venues, and deliberately narrower than what compose
/// gives them.** OANDA, Twelve Data and Alpha Vantage differ in which pairs
/// they can serve — OANDA's instrument list is direction-fixed — so there is no
/// one roster that is right for all three, and this is the intersection: the
/// single pair every FX venue quotes. `docker-compose.yml` supplies each
/// service's real roster by environment, which is where a per-venue set
/// belongs. `market-data/tests/roster_compose_agreement.rs` pins that this stays
/// a **subset** of each compose default rather than equal to it.
const DEFAULT_PRODUCTS: &str = "AUD-USD";

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
    /// Every canonical pair this collector polls, e.g. `AUD-USD`. One process
    /// serves a venue rather than a pair (see [`crate::roster`]).
    pub products: Vec<RosterEntry>,
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
            products: roster_from_env(DEFAULT_PRODUCTS)?,
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
    ///
    /// Keyed by product as well as venue, so one roster service resumes every
    /// cursor the per-pair services it replaces had saved.
    pub fn feed_name(&self, source: &str, product_id: &str) -> String {
        format!("fx:{source}:{product_id}")
    }
}

/// `backfill_days` before now, floored to a `granularity`-aligned bucket start.
fn default_start(granularity: i64, backfill_days: u64) -> i64 {
    let start = now_secs() - (backfill_days as i64) * 86_400;
    let granularity = granularity.max(1);
    start - start.rem_euclid(granularity)
}

/// Widen a poll interval when a roster's request count would exceed a venue's
/// documented daily quota.
///
/// **This is the tax a roster levies on a metered venue, and it is easy to
/// miss.** A per-pair cadence that fits comfortably for one pair is multiplied
/// by the roster size: at Alpha Vantage's four polls a day, seven pairs is 28
/// requests against an account quota of 25, so simply listing more pairs turns
/// a within-budget feed into a throttled one. The failure is quiet, too — a
/// venue answers a quota breach with an error payload, not a transport error,
/// so it surfaces as a feed that mysteriously stops advancing.
///
/// `daily_quota` is the **usable** share the caller has decided to spend, not
/// the venue's headline number: leaving room for restarts and for anything else
/// sharing the key is the caller's judgement, since only it knows what else the
/// credential is used for.
///
/// Returns the configured interval unchanged when it already fits, so a small
/// roster is never slowed down.
pub fn quota_floor_secs(configured: u64, products: usize, daily_quota: u64) -> u64 {
    let products = products.max(1) as u64;
    let quota = daily_quota.max(1);
    // The narrowest interval at which `products` feeds together stay inside
    // `quota` requests per day.
    //
    // `div_ceil`, not `/`. Truncating division rounds the *interval* down,
    // which rounds the request *count* up — over the quota, the one direction a
    // quota floor must never round. At 7 products against 20 usable requests a
    // truncating divide gives 30_240s (exactly 20/day, fine), but the general
    // case does not divide evenly and the remainder always lands on the wrong
    // side.
    let floor = 86_400_u64.saturating_mul(products).div_ceil(quota);
    configured.max(floor)
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

/// A roster resolved for a venue addressed by **bare currency code against
/// USD** — the shape the two daily batched FX collectors need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsdRoster {
    /// Currency code → the canonical product id its readings are stored under.
    /// The lookup a per-currency rate is turned back into a row with.
    pub by_currency: HashMap<String, String>,
    /// The currency codes in roster order — what the batched source is built
    /// with.
    pub currencies: Vec<String>,
}

/// Resolve `ids` into [`UsdRoster`] for `venue`, rejecting anything the venue
/// cannot serve.
///
/// **Resolved before anything connects, so a roster mistake fails startup**
/// rather than becoming a series that never appears. Both rejections below
/// would otherwise be indistinguishable from a currency the provider simply
/// does not carry — a silence that looks exactly like the venue's own coverage
/// gap, and that no amount of polling fixes:
///
/// - A pair not quoted against USD. These venues are keyed by base currency
///   (er-api in its path, Frankfurter in its query) and their adapters invert
///   each rate into USD per unit, so every pair they can serve quotes against
///   USD.
/// - Two entries naming one currency. The response is a table keyed by
///   currency, so a second entry for the same code has one reading overwrite
///   the other — with only one of the two product ids ever getting a row.
///
/// `venue` names the collector in the first error, since the operator's fix
/// differs by venue (drop the pair, or move it to a venue that quotes it).
///
/// Shared rather than written twice: this was ~20 identical lines inline in
/// each of the two collectors' `main`, which is where it was unreachable by any
/// test — a `main` body has no seam. One copy is one place to cover.
pub fn usd_quoted_currencies(venue: &str, ids: &[String]) -> Result<UsdRoster> {
    let mut by_currency: HashMap<String, String> = HashMap::with_capacity(ids.len());
    let mut currencies = Vec::with_capacity(ids.len());
    for product_id in ids {
        let (base, quote) = split_canonical(product_id)?;
        if !quote.eq_ignore_ascii_case("USD") {
            return Err(anyhow!(
                "{venue} quotes against USD, so `{product_id}` cannot be served by this venue"
            ));
        }
        // Upper-cased to match the provider's response keys, which the adapter
        // looks up verbatim. `parse_roster` already normalizes, so this is a
        // guard rail rather than the normalization itself.
        let currency = base.to_ascii_uppercase();
        if let Some(prior) = by_currency.insert(currency.clone(), product_id.clone()) {
            return Err(anyhow!(
                "`{prior}` and `{product_id}` both name {currency}, so one reading would \
                 overwrite the other"
            ));
        }
        currencies.push(currency);
    }
    Ok(UsdRoster {
        by_currency,
        currencies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_feeds::secrets::{env_var, validate_name, EnvBackend};

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
    }

    #[test]
    fn an_unresolvable_credential_names_the_feed_it_belongs_to() {
        // Deliberately NOT routed through `secret()`, which builds its chain
        // with `SecretProvider::from_env()`: on a machine that has the enclave
        // exported — the machine this feature is built for — that would consult
        // the 1Password backend for a probe name that does not exist, so a
        // plain `cargo test` would spawn `op` against a live vault and could
        // block on a biometric prompt. The assertion would still pass, so the
        // cost would be invisible. An explicit env-only chain reproduces the
        // same error path with no subprocess.
        let provider = SecretProvider::new(vec![Box::new(EnvBackend)]);
        let err = provider
            .resolve("fx-probe/absent-case")
            .context("the API credential for this feed")
            .unwrap_err()
            .to_string();
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
    fn a_roster_widens_a_metered_venues_cadence_to_stay_inside_its_quota() {
        // The motivating case: Alpha Vantage's 6-hour tick is 4 requests a day
        // for one pair, but 28 for seven — over an account quota of 25. Twenty
        // usable requests across seven pairs is one poll per pair per ~8.4h.
        assert_eq!(quota_floor_secs(21_600, 7, 20), 30_240);
        // One pair at the same cadence is already inside the quota, so the
        // configured interval stands rather than being widened for nothing.
        assert_eq!(quota_floor_secs(21_600, 1, 20), 21_600);
    }

    #[test]
    fn a_generous_quota_never_slows_a_feed_down() {
        // OANDA documents 100 requests/second, so nothing a roster does to the
        // request count comes near it and the configured cadence must survive.
        assert_eq!(quota_floor_secs(60, 7, 8_640_000), 60);
    }

    #[test]
    fn the_floor_degrades_safely_rather_than_dividing_by_zero() {
        // A misconfigured quota or an empty roster must not panic or produce a
        // zero interval, which would poll as fast as the loop allows.
        assert!(quota_floor_secs(60, 0, 0) >= 60);
        assert!(quota_floor_secs(0, 1, 1) > 0);
    }

    #[test]
    fn the_feed_name_pairs_a_cursor_with_the_rows_it_wrote() {
        let cfg = FxConfig {
            database_url: String::new(),
            base_url: String::new(),
            products: crate::roster::parse_roster("AUD-USD,EUR-USD").unwrap(),
            granularity_secs: 60,
            backfill_start_secs: 0,
            max_buckets_per_request: 5_000,
            poll_interval_secs: 60,
        };
        assert_eq!(cfg.feed_name("oanda", "AUD-USD"), "fx:oanda:AUD-USD");
        // The cursor key is per pair, not per process: a roster service and the
        // per-pair service it replaces name the same cursor, which is what
        // makes the split a no-op for resume.
        assert_eq!(cfg.feed_name("oanda", "EUR-USD"), "fx:oanda:EUR-USD");
    }

    fn ids(spec: &str) -> Vec<String> {
        spec.split(',').map(str::to_string).collect()
    }

    #[test]
    fn a_usd_quoted_roster_resolves_to_currencies_and_a_reverse_lookup() {
        let roster = usd_quoted_currencies("er-api", &ids("AUD-USD,NGN-USD")).unwrap();
        // Roster order is preserved: this is what the batched source is built
        // with, and the silence watch reports against the same sequence.
        assert_eq!(roster.currencies, ["AUD", "NGN"]);
        // The reverse lookup is what turns a per-currency rate back into a row
        // under the canonical id — the join key nothing else can supply.
        assert_eq!(roster.by_currency["AUD"], "AUD-USD");
        assert_eq!(roster.by_currency["NGN"], "NGN-USD");
    }

    #[test]
    fn a_lower_cased_entry_is_upper_cased_to_match_the_response_keys() {
        // `parse_roster` normalizes already, so this is the guard rail rather
        // than the normalization — pinned because the adapter looks the code up
        // verbatim in the provider's table, where a miss is silent.
        let roster = usd_quoted_currencies("er-api", &ids("eur-usd")).unwrap();
        assert_eq!(roster.currencies, ["EUR"]);
        // Only the lookup KEY is upper-cased; the product id is carried through
        // byte-for-byte. This pins that local contract, not a storage property:
        // `parse_roster` upper-cases every id before one reaches this function,
        // so a lower-case id never arrives in production and this test cannot
        // say anything about what `spot_ticks.product_id` holds.
        assert_eq!(roster.by_currency["EUR"], "eur-usd");
    }

    #[test]
    fn a_pair_not_quoted_against_usd_fails_startup() {
        // These venues invert every rate on a USD base, so a non-USD quote is a
        // roster mistake no amount of polling fixes — and left unchecked it
        // looks exactly like a currency the provider does not carry.
        let err = usd_quoted_currencies("frankfurter", &ids("EUR-GBP"))
            .expect_err("a non-USD quote must not resolve");
        let rendered = err.to_string();
        // The venue names itself, because the operator's fix differs by venue.
        assert!(rendered.contains("frankfurter"), "{rendered}");
        assert!(rendered.contains("EUR-GBP"), "{rendered}");
    }

    #[test]
    fn two_entries_naming_one_currency_fail_startup() {
        // The response is a table keyed by currency, so a duplicate has one
        // reading overwrite the other and only one product id ever gets a row.
        // Rejecting names BOTH ids, since the operator has to pick.
        let err = usd_quoted_currencies("er-api", &ids("EUR-USD,EUR-USD"))
            .expect_err("a duplicated currency must not resolve");
        let rendered = err.to_string();
        assert!(rendered.contains("EUR"), "{rendered}");
        assert!(rendered.contains("overwrite"), "{rendered}");
        // The case-folded duplicate is caught too — the upper-casing above is
        // what makes `eur-usd` and `EUR-USD` collide rather than pass as two.
        assert!(usd_quoted_currencies("er-api", &ids("EUR-USD,eur-usd")).is_err());
    }

    #[test]
    fn a_malformed_id_is_rejected_before_the_usd_check() {
        // `split_canonical`'s guard still runs first: a non-canonical id must
        // not reach the quote comparison and be reported as a USD problem.
        // A vendor's own spelling, which is the realistic mistake: OANDA writes
        // the pair `EUR_USD`, so it carries no `-` to split on at all.
        //
        // Asserted on the rendered message rather than with a bare `is_err()`:
        // both inputs are errors under EITHER ordering, so `is_err()` alone
        // could not fail if the USD comparison ever ran first — which is the
        // property this test is named for. `split_canonical`'s message names
        // the canonical-symbol problem and says nothing about USD quoting.
        for bad in ["EUR_USD", "EUROS-USD"] {
            let err = usd_quoted_currencies("er-api", &ids(bad))
                .unwrap_err()
                .to_string();
            assert!(err.contains("canonical"), "{bad}: {err}");
            assert!(!err.contains("quotes against USD"), "{bad}: {err}");
        }
    }

    #[test]
    fn an_empty_roster_resolves_to_nothing_rather_than_erroring() {
        // Not a startup failure here: `parse_roster` is what rejects an empty
        // spec, and duplicating that judgement would give one mistake two
        // different error messages depending on which layer caught it.
        let roster = usd_quoted_currencies("erapi", &[]).unwrap();
        assert!(roster.currencies.is_empty());
        assert!(roster.by_currency.is_empty());
    }
}
