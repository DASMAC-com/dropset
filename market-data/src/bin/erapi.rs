//! The er-api daily FX collector: one batched keyless request prices every
//! roster currency, and each reading lands in `spot_ticks` stamped with the
//! provider's own refresh instant (docs/data-feeds.md §9).
//!
//! **This is the roster's widest keyless source, and NGN is why it is wired.**
//! Neither of the other keyless FX tiers prices NGN — Pyth Hermes catalogues it
//! but has never published a price for it, and the ECB reference set behind
//! Frankfurter omits it entirely — so until this collector ran, that currency
//! rested on two keyed vendors alone. The provider also blends central-bank and
//! commercial sources and lists no code with fewer than three upstreams, so it
//! is a differently-constructed estimate rather than a second render of the ECB
//! fix.
//!
//! `observed_at` is the provider's `last_update`, not the poll second, and that
//! is load-bearing twice over. This is a **daily** snapshot refreshed near
//! 00:00 UTC, so the instant a reading was fetched is not the instant it
//! describes — often by many hours — and stamping at fetch time would record a
//! stale value as fresh. It is also what makes a re-poll idempotent: the same
//! snapshot re-fetched carries the same instant, so it lands on the primary key
//! instead of writing a second row for one observation.
//!
//! **The cadence is hourly, deliberately not the shared tick interval.** The
//! other `spot_ticks` collectors poll every 15 s, which is right for a streaming
//! tape and wrong here: this venue publishes no rate limit and refreshes once a
//! day, so that cadence would spend ~5,700 requests a day on a keyless endpoint
//! re-reading a value the provider has already said will not change (it hands
//! back the instant of the next refresh in every response). Hourly picks a new
//! snapshot up within an hour of publication for 24 requests, and the idempotent
//! insert absorbs the redundant ones rather than accumulating duplicate rows.
//!
//! **License — internal use only, and it binds any new read surface.** The
//! open-access endpoint permits caching and commercial currency-conversion use
//! but **prohibits re-distribution**, and requires attribution wherever the rates
//! are shown. Storing readings here and consuming them to compute a fair value
//! is squarely the permitted use; surfacing them *raw* through the public indexer
//! API or an externally shared dashboard is not — and both read this same store.

use dropset_feeds::{
    connect, run,
    venues::{ErApiSnapshot, ErApiSource},
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
/// no compile error. The venue is written `er-api` in prose and `erapi`
/// everywhere a key is parsed.
const SOURCE: &str = "erapi";

/// The cursor key this collector's sink is wired with. A latest-rates endpoint
/// has no resume position, so nothing is written under it.
const FEED: &str = "ticks:erapi";

/// Every non-USD fiat on the roster: this venue prices its whole table in one
/// request, so the roster costs nothing to widen and there is no reason to carry
/// less than all of it.
///
/// Fourteen currencies, matching the fiat set seeded in `currency_kinds` less
/// `USD` itself — which is the quote leg here rather than a product. That
/// coupling is not checked mechanically; a fiat added to the roster and not added
/// here is simply a series this venue never records.
const DEFAULT_PRODUCTS: &str = "AUD-USD,BRL-USD,CAD-USD,CHF-USD,EUR-USD,GBP-USD,IDR-USD,\
                                JPY-USD,MXN-USD,MYR-USD,NGN-USD,SGD-USD,TRY-USD,ZAR-USD";

/// How many polls a configured currency may stay unpriced before it is reported
/// as a roster mistake rather than a venue gap.
///
/// Lower than the streaming collectors' threshold because the evidence is
/// stronger per poll: this venue returns its **whole** table on every request
/// rather than answering a per-symbol query, so a currency absent from a
/// complete response is one the provider does not carry. The threshold is still
/// not `1`, because a single truncated or partial response should not be allowed
/// to cry wolf. At the hourly cadence above this reports a roster typo within a
/// few hours, which is well inside the 24-hour latency the data itself carries.
const SILENCE_THRESHOLD: u32 = 3;

/// How far ahead of this host's clock the provider's refresh instant may sit
/// before it is treated as bogus rather than as "just published".
///
/// The same minute the maker-bot's `MAX_VENUE_CLOCK_SKEW` allows, and it is here
/// for a sharper reason: this is the path that **persists**. `spot_ticks` has no
/// plausibility constraint on `observed_at` (only `confidence` is checked), the
/// column is part of the insert's conflict key, and
/// `db-schema/migrations/0009_instruments.sql` derives each feed's last-seen
/// instant as `max(observed_at)`. So one far-future stamp would pin that
/// product's freshness forever, and — because the insert is
/// `ON CONFLICT … DO NOTHING` — the correct later snapshot, whose instant is
/// smaller, could never supersede it. Writing such a row is not recoverable in
/// place; refusing it is one comparison.
const MAX_CLOCK_SKEW_SECS: i64 = 60;

/// The instant a reading is attributed to: the provider's own refresh instant
/// where it is plausible, else the poll second.
///
/// Both rejections degrade to the poll second rather than to the epoch or to a
/// far-future date. That costs the idempotency the ordinary path enjoys — a
/// poll-stamped row is new on every poll — which is the honest trade: a snapshot
/// whose own timestamp is nonsense has no instant to dedup on, and duplicate
/// rows are recoverable where a permanently-pinned bogus one is not.
fn observation_instant(last_update: i64, poll_secs: i64) -> i64 {
    if last_update <= 0 || last_update.saturating_sub(poll_secs) > MAX_CLOCK_SKEW_SECS {
        poll_secs
    } else {
        last_update
    }
}

const DEFAULTS: TickDefaults = TickDefaults {
    base_url: "https://open.er-api.com",
    // See the module note: a daily snapshot, so hourly rather than the shared
    // 15 s tick interval.
    poll_interval_secs: 3_600,
};

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
    // The request is keyed by base currency in its path (`/v6/latest/USD`) and
    // the adapter inverts each rate into USD per unit, so every pair this venue
    // can serve quotes *against USD*. An entry that does not is a roster mistake
    // no amount of polling will fix, and it would otherwise look exactly like a
    // currency the provider does not carry.
    let mut by_currency: HashMap<String, String> = HashMap::with_capacity(ids.len());
    let mut currencies = Vec::with_capacity(ids.len());
    for product_id in &ids {
        let (base, quote) = split_canonical(product_id)?;
        if !quote.eq_ignore_ascii_case("USD") {
            anyhow::bail!(
                "er-api quotes against USD, so `{product_id}` cannot be served by this venue"
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
        "erapi daily collector starting"
    );

    let source = ErApiSource::new(&cfg.base_url, currencies)?;
    let source = TickSource::new(source, move |snap: &ErApiSnapshot, poll_secs| {
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
                    // Prefer the provider's refresh instant, bounded in both
                    // directions — see `observation_instant`.
                    observed_at: observation_instant(snap.last_update, poll_secs),
                    price: *rate,
                    // This venue publishes no uncertainty. `None` records that
                    // it has no confidence notion, which a zero would misread as
                    // perfect certainty.
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

    #[test]
    fn a_plausible_provider_instant_is_what_the_reading_is_attributed_to() {
        let poll = 1_788_307_351;
        // The ordinary case: a snapshot published ~21.7 hours ago, which is what
        // this venue actually serves.
        let published = poll - 78_131;
        assert_eq!(observation_instant(published, poll), published);
        // Sub-minute skew ahead of us is tolerated rather than discarded.
        assert_eq!(observation_instant(poll + 30, poll), poll + 30);
        assert_eq!(observation_instant(poll, poll), poll);
    }

    #[test]
    fn an_implausible_provider_instant_degrades_to_the_poll_second() {
        let poll = 1_788_307_351;
        // A missing or zeroed stamp must not attribute the reading to the epoch.
        assert_eq!(observation_instant(0, poll), poll);
        assert_eq!(observation_instant(-1, poll), poll);
        // A far-future stamp is nonsense, not freshness. This is the case that
        // matters most: `spot_ticks` has no plausibility constraint on
        // `observed_at`, the insert is `ON CONFLICT … DO NOTHING`, and the
        // instruments view reads `max(observed_at)` — so such a row would pin
        // the feed's last-seen instant permanently and could never be
        // superseded by the correct later snapshot.
        assert_eq!(observation_instant(poll + 10_000_000, poll), poll);
        assert_eq!(observation_instant(i64::MAX, poll), poll);
        // Saturating arithmetic: no wrap, no panic, at either extreme.
        assert_eq!(observation_instant(i64::MIN, poll), poll);
    }
}
