//! The ECB / Frankfurter daily FX collector: one batched keyless request
//! prices every roster currency, and each reading lands in `spot_ticks`
//! stamped with the ECB reference date rather than the poll second
//! (docs/data-feeds.md §9).
//!
//! **This is the roster's reference fix, and it is deliberately not a lead.**
//! Frankfurter republishes the ECB reference rates — a single administered
//! daily fix, not a market tape — so it earns its slot as breadth and as the
//! anchor fallback, never as a live-quote input. It is also *the* ECB fix
//! rather than one of several: nothing else in the roster may be wired as a
//! second ECB source, because two renders of one fix would read to the
//! consensus filter as two agreeing venues.
//!
//! `observed_at` is midnight UTC of the provider's reference date, not the poll
//! second, and that is load-bearing twice over — the same reasoning the er-api
//! collector records at length, for the same venue class. These rates change
//! once a business day, so the instant a reading was fetched is not the instant
//! it describes; over a weekend the gap is more than two days, and stamping at
//! fetch time would record a Friday fix as fresh on Sunday night. It is also
//! what makes a re-poll idempotent: the same fix re-fetched carries the same
//! date, so it lands on the primary key instead of writing a second row for one
//! observation.
//!
//! **Midnight rather than the nominal 16:00 CET publication.** Midnight is
//! conservative in the only direction that is safe — a reading can look staler
//! than it is, never fresher — and it keeps an ECB schedule constant and its
//! DST dependency out of the code for no decision value. The consequence to
//! know: for the ~7 hours between midnight UTC and the actual fix, a reading is
//! attributed slightly earlier than it was published. Any staleness band that
//! admits a daily feed at all already tolerates that offset.
//!
//! **NGN is absent from the roster below, and that is the venue, not an
//! omission.** The ECB reference set does not carry it: a live probe of all 14
//! roster currencies returns 13, NGN alone missing. Listing it anyway would buy
//! a series that never arrives and a silence watch that fires forever, so the
//! currency rests on er-api and the two keyed vendors. This is the one place
//! the two daily keyless tiers do not overlap.
//!
//! **The cadence is hourly, deliberately not the shared tick interval.** The
//! other `spot_ticks` collectors poll every 15 s, which is right for a
//! streaming tape and wrong here: this venue publishes no rate limit and
//! refreshes once a business day, so that cadence would spend ~5,700 requests a
//! day on a keyless endpoint re-reading a value that cannot have moved. Hourly
//! picks a new fix up within an hour of publication for 24 requests, and the
//! idempotent insert absorbs the redundant ones rather than accumulating
//! duplicate rows.

use dropset_feeds::{
    connect, run,
    venues::{FrankfurterSnapshot, FrankfurterSnapshots},
    RunConfig, Sink, StoreSink,
};
use dropset_market_data::{
    fx::split_canonical,
    instruments::register as register_instruments,
    roster::{canonical_only, roster_from_env},
    ticks::{SilenceWatch, Tick, TickConfig, TickDefaults, TickSource, TickWriter},
};
use std::collections::HashMap;
use std::time::Duration;

/// The value written to `spot_ticks.source`.
///
/// A literal, like every sibling collector's, and deliberately **not** bound to
/// the adapter's `FEED_NAME`. This value is schema-facing data: it is the domain
/// of the dashboard's Source variable and the key every stored row is filed
/// under, so a rename inside `feeds/` must not silently re-label new rows and
/// orphan the existing ones — which is exactly what deriving it would do, with
/// no compile error.
const SOURCE: &str = "frankfurter";

/// The cursor key this collector's sink is wired with. A latest-rates endpoint
/// has no resume position, so nothing is written under it.
const FEED: &str = "ticks:frankfurter";

/// Every roster currency the ECB reference set carries: this venue prices its
/// whole table in one request, so the roster costs nothing to widen and there
/// is no reason to carry less than all of it.
///
/// Thirteen currencies — the fourteen fiats seeded in `currency_kinds`, less
/// `USD` itself (the quote leg here rather than a product) and less **NGN**,
/// which this venue does not carry at all. See the module note: the omission is
/// measured, not assumed.
const DEFAULT_PRODUCTS: &str = "AUD-USD,BRL-USD,CAD-USD,CHF-USD,EUR-USD,GBP-USD,IDR-USD,\
                                JPY-USD,MXN-USD,MYR-USD,SGD-USD,TRY-USD,ZAR-USD";

/// How many polls a configured currency may stay unpriced before it is reported
/// as a roster mistake rather than a venue gap.
///
/// Matches the er-api collector, for the same reason: this venue returns its
/// **whole** table on every request rather than answering a per-symbol query,
/// so a currency absent from a complete response is one the provider does not
/// carry. The threshold is still not `1`, because a single truncated response
/// should not be allowed to cry wolf. At the hourly cadence this reports a
/// roster typo within a few hours — well inside the latency the data carries.
const SILENCE_THRESHOLD: u32 = 3;

/// How far ahead of this host's clock the reference date may sit before it is
/// treated as bogus rather than as "just published".
///
/// The same minute the er-api collector and the maker-bot's
/// `MAX_VENUE_CLOCK_SKEW` allow, and it is here for the sharper reason er-api
/// records: this is the path that **persists**. `spot_ticks` has no
/// plausibility constraint on `observed_at`, the column is part of the insert's
/// conflict key, and `db-schema/migrations/0009_instruments.sql` derives each
/// feed's last-seen instant as `max(observed_at)`. So one far-future stamp
/// would pin this feed's freshness forever and — because the insert is
/// `ON CONFLICT … DO NOTHING` — the correct later fix, whose date is smaller,
/// could never supersede it.
///
/// A whole minute is generous here, since midnight UTC of a plausible reference
/// date is always in the past. It is deliberately not tightened to zero: the
/// two daily collectors agreeing on one bound is worth more than a bound fitted
/// to this venue's arithmetic, and a host clock a few seconds behind must not
/// manufacture a degrade.
const MAX_CLOCK_SKEW_SECS: i64 = 60;

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: "https://api.frankfurter.dev/v1",
    // See the module note: a daily fix, so hourly rather than the shared 15 s
    // tick interval.
    poll_interval_secs: 3_600,
};

/// The instant a reading is attributed to: midnight UTC of the provider's
/// reference date where there is a plausible one, else the poll second.
///
/// **The two directions are not bounded symmetrically, deliberately** — the
/// same asymmetry er-api carries. Forward is bounded tightly
/// (`MAX_CLOCK_SKEW_SECS`); backward is bounded only at zero, so an absurdly
/// old date is accepted and stored. That follows the threat: an ancient stamp
/// is self-evidently ancient to every reader and cannot pin `max(observed_at)`,
/// whereas a far-future one both looks current and can never be superseded.
///
/// A `None` date — the provider omitted the field, or sent one that does not
/// parse — degrades the same way. Both degradations cost the idempotency the
/// ordinary path enjoys, since a poll-stamped row is new on every poll. That is
/// the honest trade: a fix whose own date is missing or nonsense has no instant
/// to dedup on, and duplicate rows are recoverable where a permanently-pinned
/// bogus one is not. The substitution is logged by the caller, since nothing in
/// the stored row records that it happened.
fn observation_instant(reference_date: Option<i64>, poll_secs: i64) -> i64 {
    match reference_date {
        Some(date) if date > 0 && date.saturating_sub(poll_secs) <= MAX_CLOCK_SKEW_SECS => date,
        _ => poll_secs,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cfg = TickConfig::from_env(&DEFAULTS)?;
    let products = roster_from_env(DEFAULT_PRODUCTS)?;
    // Canonical ids reach this venue as bare currency codes, so a pinned venue
    // spelling has nowhere to go and is rejected rather than silently dropped.
    let ids = canonical_only(&products)?;

    // Resolve the whole roster before connecting: a malformed or unquotable
    // entry must fail startup rather than become a series that never appears.
    //
    // The request is keyed by base currency (`?base=USD`) and the adapter
    // inverts each rate into USD per unit, so every pair this venue can serve
    // quotes *against USD*. An entry that does not is a roster mistake no
    // amount of polling will fix, and it would otherwise look exactly like a
    // currency the provider does not carry.
    let mut by_currency: HashMap<String, String> = HashMap::with_capacity(ids.len());
    let mut currencies = Vec::with_capacity(ids.len());
    for product_id in &ids {
        let (base, quote) = split_canonical(product_id)?;
        if !quote.eq_ignore_ascii_case("USD") {
            anyhow::bail!(
                "frankfurter quotes against USD, so `{product_id}` cannot be served by this venue"
            );
        }
        // Upper-cased to match the provider's response keys, which the adapter
        // looks up verbatim. `parse_roster` already normalizes, so this is a
        // guard rail rather than the normalization itself.
        let currency = base.to_ascii_uppercase();
        if let Some(prior) = by_currency.insert(currency.clone(), product_id.clone()) {
            anyhow::bail!(
                "`{prior}` and `{product_id}` both name {currency}, so one reading would \
                 overwrite the other"
            );
        }
        currencies.push(currency);
    }

    let pool = connect(&cfg.database_url).await?;
    dropset_db_schema::require_schema(&pool).await?;
    register_instruments(&pool, SOURCE, &ids).await?;
    tracing::info!(
        products = %ids.join(","),
        poll_secs = cfg.poll_interval_secs,
        "frankfurter daily collector starting"
    );

    let source = FrankfurterSnapshots::new(&cfg.base_url, currencies)?;
    let source = TickSource::new(source, move |snap: &FrankfurterSnapshot, poll_secs| {
        // Resolved once per poll, not per currency: the date is a property of
        // the fix, and so is the warning below.
        let observed_at = observation_instant(snap.reference_date, poll_secs);
        if Some(observed_at) != snap.reference_date {
            // Say so, because the substitution is otherwise invisible and its
            // consequence is the opposite of obvious: the rows still look like
            // ordinary hourly observations, so `max(observed_at)` tracks the
            // poll second and the store reports this feed as perfectly FRESH
            // under exactly the condition a staleness band would drop it for.
            // Nothing downstream can tell a substituted instant from a real
            // one, so this log line is the only signal.
            tracing::warn!(
                reference_date = ?snap.reference_date,
                attributed_to = observed_at,
                "frankfurter reference date is missing or implausible; attributing this \
                 poll to the poll second, which forfeits the idempotent insert"
            );
        }
        snap.rates
            .iter()
            .filter_map(|(currency, rate)| {
                // A rate for a currency outside the roster cannot be stored:
                // `spot_ticks` keys on the canonical product id and only the
                // roster says what that is. The adapter already filters to the
                // currencies it was built with, so this is unreachable today
                // rather than a silent drop of wanted data.
                let product_id = by_currency.get(currency)?;
                Some(Tick {
                    product_id: product_id.clone(),
                    // Midnight UTC of the reference date where it is plausible
                    // — see `observation_instant`, resolved above.
                    observed_at,
                    price: *rate,
                    // This venue publishes no uncertainty. `None` records that
                    // it has no confidence notion, which a zero would misread
                    // as perfect certainty.
                    confidence: None,
                })
            })
            .collect()
    })
    // Driven once per poll by the adapter, not from the mapping above — see
    // `TickSource::watching`.
    .watching(SilenceWatch::new(ids, SILENCE_THRESHOLD));

    let sinks: Vec<Box<dyn Sink<Tick>>> = vec![Box::new(StoreSink::new(
        pool,
        FEED,
        TickWriter::new(SOURCE),
    ))];
    let run_cfg = RunConfig {
        poll_interval: Duration::from_secs(cfg.poll_interval_secs),
        ..RunConfig::default()
    };
    run(source, sinks, run_cfg).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Midnight UTC on 2026-09-08, the reference date the live endpoint
    /// returned when this collector was written.
    const REFERENCE_DATE: i64 = 1_788_825_600;

    #[test]
    fn a_plausible_reference_date_is_what_the_reading_is_attributed_to() {
        // The ordinary case: polled during the day whose fix this is, so the
        // stamp sits hours behind the poll.
        let poll = REFERENCE_DATE + 47_000;
        assert_eq!(
            observation_instant(Some(REFERENCE_DATE), poll),
            REFERENCE_DATE
        );
        // Polled over a weekend, when the fix is the Friday one and no new
        // fix exists. The stamp must stay Friday's rather than creep forward —
        // this is the case that makes the feed's real age legible.
        let sunday = REFERENCE_DATE + 2 * 86_400;
        assert_eq!(
            observation_instant(Some(REFERENCE_DATE), sunday),
            REFERENCE_DATE
        );
        // A poll landing exactly on midnight is not skew.
        assert_eq!(
            observation_instant(Some(REFERENCE_DATE), REFERENCE_DATE),
            REFERENCE_DATE
        );
        // The boundary itself, both sides. Pinned because the er-api half
        // admits exactly 60 s too, and the two daily collectors agreeing at the
        // edge is the property worth holding.
        let just_behind = REFERENCE_DATE - 60;
        assert_eq!(
            observation_instant(Some(REFERENCE_DATE), just_behind),
            REFERENCE_DATE
        );
        assert_eq!(
            observation_instant(Some(REFERENCE_DATE), just_behind - 1),
            just_behind - 1
        );
    }

    #[test]
    fn a_missing_or_implausible_reference_date_degrades_to_the_poll_second() {
        let poll = REFERENCE_DATE + 47_000;
        // No date at all — the provider omitted it, or it did not parse. This
        // must not attribute the reading to the epoch.
        assert_eq!(observation_instant(None, poll), poll);
        // A zeroed or negative stamp is caught by the `> 0` arm.
        assert_eq!(observation_instant(Some(0), poll), poll);
        assert_eq!(observation_instant(Some(-1), poll), poll);
        // A far-future date is nonsense, not freshness. This is the case that
        // matters most: `spot_ticks` has no plausibility constraint on
        // `observed_at`, the insert is `ON CONFLICT … DO NOTHING`, and the
        // instruments view reads `max(observed_at)` — so such a row would pin
        // this feed's last-seen instant permanently and could never be
        // superseded by the correct later fix.
        assert_eq!(observation_instant(Some(poll + 10_000_000), poll), poll);
        // `i64::MAX` is the case the saturating subtraction exists for: a plain
        // `-` would overflow here.
        assert_eq!(observation_instant(Some(i64::MAX), poll), poll);
        // `i64::MIN` is caught by the `> 0` arm, which short-circuits before
        // the subtraction runs — so this pins the guard, not the arithmetic.
        assert_eq!(observation_instant(Some(i64::MIN), poll), poll);
    }

    #[test]
    fn the_default_roster_omits_the_currency_this_venue_does_not_carry() {
        // Measured against the live endpoint: all 14 roster currencies asked
        // for, 13 returned. Wiring NGN here would buy a permanently silent
        // series and a silence watch that fires forever.
        assert!(!DEFAULT_PRODUCTS.contains("NGN"));
        assert_eq!(DEFAULT_PRODUCTS.split(',').count(), 13);
        // Every entry quotes against USD, which `main` enforces at startup —
        // asserted here so a typo in the constant fails the suite rather than
        // the container.
        assert!(DEFAULT_PRODUCTS.split(',').all(|p| p.ends_with("-USD")));
    }
}
