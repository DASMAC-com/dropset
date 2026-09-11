//! Venue adapters (`http` feature) — the concrete [`crate::Source`]s, one
//! module per venue.
//!
//! An adapter lives **here, not in whichever app needed it first**
//! (docs/data-feeds.md §4), so a venue is written once and consumed by both
//! sink shapes: a collector wires it to a store sink and persists the history,
//! a bot wires the same source to a forward sink and quotes off it. Nothing in
//! this module knows which.
//!
//! Two adapter shapes, decided by the venue's endpoint rather than by taste:
//!
//! - **Batched quote venues** — one request prices many symbols, so the source
//!   is built with the whole symbol set and yields a [`Quotes`] map per poll.
//!   [`coingecko`], [`coinmarketcap`], [`frankfurter`], and [`kraken`] are the
//!   four today. Batching is the per-venue budget's main lever (§10): one poll
//!   for N markets, not N polls.
//! - **Per-product venues** — the endpoint is keyed by a single product, so
//!   batching is not on offer and one source covers one product. [`coinbase`]
//!   is both cases: its candles endpoint pages its own backfill, and its
//!   ticker endpoint yields one spot price.
//! - **Batched venues richer than a price** — [`pyth`] and [`erapi`] batch like
//!   the first group but carry more than a rate, so neither rides [`Quotes`]'
//!   bare `f64`. Pyth yields a confidence half-width and a publish time
//!   alongside each rate, which is what makes it the FX anchor's *primary* tier
//!   rather than another fallback; [`erapi`] yields the provider's own refresh
//!   instants, without which a once-daily snapshot would be stored as though it
//!   described the moment it happened to be fetched.
//!
//! **The batched-poll contract — stated here, not encoded in a trait.** Every
//! batched quote venue above exposes exactly one inherent `poll` covering its
//! full roster, and **omits** symbols the venue does not quote rather than
//! failing the whole batch: a roster with one unlisted token still prices the
//! rest. That `poll` stays **public** alongside the adapter's [`crate::Source`]
//! impl, whose `next` is just the same poll wrapped in a batch, so one adapter
//! drives the runner *and* answers a caller that wants a single synchronous
//! reading (a `--dry-run` reachability check) with no runner at all.
//!
//! The contract is a convention held by review, and deliberately not a trait. A
//! venue's symbol key is its own — CoinGecko slugs are `String`, CoinMarketCap
//! listing ids are `u32` — so no `dyn` collection could ever unify these
//! adapters, and nothing consumes them generically: every caller holds a
//! concrete source. The polymorphic seam for ingestion already lives one layer
//! up, at [`crate::Source`] / [`crate::Sink`], and a venue-level trait would
//! only duplicate it with incompatible types while signalling a polymorphism
//! that does not exist.
//!
//! If a future uniform poller (one poller per venue, sharing that venue's
//! budget) wants a common consumer, it designs the abstraction there, against
//! its own real needs — the heterogeneous symbol keys mean such a thing wants
//! closure- or [`crate::Source`]-shaped erasure rather than a bare venue trait.
//!
//! Each adapter splits its decode out into free `parse_*` functions, which need
//! no network: they are unit tested against captured responses, so a venue's
//! JSON shape stays covered without anything reaching the venue itself. Only
//! the transport half needs a network, and nothing here tests that.
//!
//! **Credentials arrive by injection, never by an environment read in here.**
//! A keyed adapter takes its key as a constructor argument
//! ([`oanda::OandaCandles::resume`]) so the caller decides where the secret came
//! from — a process environment today, a secrets provider later — and no adapter
//! has to change when that answer does. Most adapters here need none: every
//! venue the maker's cascade reads is keyless, [`coinmarketcap`] deliberately so
//! (docs/data-feeds.md §4 — its keyless route trades a monthly credit quota for
//! a plain rate).
//!
//! **Every adapter states its own request floor, sized to its venue's
//! documented limit** (docs/data-feeds.md §10 tabulates them). The shared
//! client's 250 ms default is right for only two venues, and the runner
//! tight-loops while a source backfills — so a venue that inherits the default
//! without checking is a venue that will be throttled the first time anything
//! pages. Each module's `MIN_REQUEST_INTERVAL` carries the documented number it
//! was derived from, and a unit test asserts the arithmetic still holds.

use anyhow::{anyhow, Result};
use std::collections::HashMap;

// Each venue rides its own transport's gate, not the module's — the `Quotes`
// alias below is transport-free, so a future streaming venue lands here too.
#[cfg(feature = "http")]
pub mod alphavantage;
#[cfg(feature = "http")]
pub mod coinbase;
#[cfg(feature = "http")]
pub mod coingecko;
#[cfg(feature = "http")]
pub mod coinmarketcap;
#[cfg(feature = "http")]
pub mod erapi;
#[cfg(feature = "http")]
pub mod frankfurter;
#[cfg(feature = "http")]
pub mod kraken;
#[cfg(feature = "http")]
pub mod oanda;
#[cfg(feature = "http")]
pub mod pyth;
#[cfg(feature = "http")]
pub mod twelvedata;

#[cfg(feature = "http")]
pub use alphavantage::AlphaVantageDaily;
#[cfg(feature = "http")]
pub use coinbase::{CoinbaseCandles, CoinbaseTicker};
#[cfg(feature = "http")]
pub use coingecko::CoinGeckoSource;
#[cfg(feature = "http")]
pub use coinmarketcap::CmcSource;
#[cfg(feature = "http")]
pub use erapi::{ErApiSnapshot, ErApiSource};
#[cfg(feature = "http")]
pub use frankfurter::{FrankfurterSnapshot, FrankfurterSnapshotSource, FrankfurterSource};
#[cfg(feature = "http")]
pub use kraken::KrakenSource;
#[cfg(feature = "http")]
pub use oanda::OandaCandles;
#[cfg(feature = "http")]
pub use pyth::{FxQuote, PythFeed, PythHermesSource};
#[cfg(feature = "http")]
pub use twelvedata::TwelveDataCandles;

/// A single closed OHLCV candle — the record every candle source yields, and
/// the row shape `cex_prices` stores.
///
/// It lives here rather than in one venue's module because it is the shared
/// currency between candle adapters and the collectors that persist them:
/// [`coinbase::CoinbaseCandles`] and [`oanda::OandaCandles`] both produce it,
/// and a store writer is written once against it rather than once per venue.
///
/// The pair, source, and granularity live on the consumer's writer (they are
/// constant per feed), so a record carries only what varies bucket to bucket.
/// `volume` is whatever the venue means by it — traded size on a CEX, tick
/// count on an FX venue, and `0.0` where the venue publishes none at all — so
/// it is comparable only *within* a source, never across two.
#[derive(Clone, Debug, PartialEq)]
pub struct Candle {
    /// Epoch-second bucket open.
    pub bucket_start: i64,
    pub low: f64,
    pub high: f64,
    pub open: f64,
    pub close: f64,
    pub volume: f64,
}

impl Candle {
    /// Accept this bar only if every price is finite and positive and the
    /// bucket's high sits at or above its low; otherwise say why, so a caller
    /// can **drop** the bar at intake rather than pass it to a store.
    ///
    /// One rule, in one place, so an adapter cannot silently omit it — nothing
    /// makes a missing per-adapter guard visible at the call site.
    ///
    /// **Returning the candle rather than `()`** is what keeps a call site to
    /// one combinator: a decode already yielding `Result<Candle>` chains
    /// straight through `and_then(Candle::validated)`, and an adapter that
    /// builds the record inline calls it on the value it just built.
    ///
    /// # What it fronts, and why it is not the only guard
    ///
    /// `cex_prices` is **gaining** CHECK constraints that assert these same
    /// three things, and once they land they are the authority: a constraint
    /// cannot be bypassed by adding a writer, and this method can — it guards
    /// the adapters that call it and nothing else. No migration filename is
    /// named here on purpose. That migration is not merged yet and its number
    /// can still move, so a citation would be a forward reference to something
    /// this crate cannot see.
    ///
    /// The reason to *also* check here is the **cost of a rejection**, not any
    /// doubt about coverage. The store writes a batch in one transaction and
    /// the cursor advances only after a successful commit, so a refused bar
    /// takes its whole batch down with it and stops that venue's collector
    /// until the code or the data changes.
    ///
    /// So this is the lenient layer and the constraint is the strict one, which
    /// is the right way round: intake knows which bar it is and can skip it,
    /// while the database knows only that a batch is bad. What the lenient
    /// layer costs is that a dropped bucket is *usually* **not** re-fetched,
    /// since the cursor advances past its window regardless. The exception is a
    /// source that re-serves its whole history every poll — see
    /// [`alphavantage`] — where a bar dropped over a transient glitch does come
    /// back on the next one.
    ///
    /// # Why finiteness is a separate clause from positivity
    ///
    /// `!is_finite()` rejects `NaN` and both infinities, and `<= 0.0` rejects
    /// zero and negatives. Neither clause implies the other, and in Rust the
    /// gap is on the opposite side from the SQL one worth knowing about: here
    /// `NaN <= 0.0` is *false* (every `NaN` comparison is), so a positivity
    /// test alone would **admit** `NaN`, whereas in Postgres `NaN > 0` holds
    /// because `NaN` sorts above every float. Different mechanisms, same
    /// conclusion — the conjunction is what means "finite and positive", in
    /// either language.
    ///
    /// `volume` is deliberately unchecked, matching the column: zero volume is
    /// routine — two wired sources publish none at all and their rows carry
    /// `0.0` — so there is no positivity invariant to assert. A non-finite
    /// volume is not currently reachable, since the sources that carry a real
    /// one hand over a parsed float and the rest hard-code `0.0`.
    ///
    /// # Deliberately not the full OHLC ordering
    ///
    /// That `open` and `close` each sit inside `[low, high]` is a coherent
    /// stronger claim, and it is **not** asserted here — for the same reason
    /// the schema declines it: it is a claim about how every present and future
    /// adapter assembles a bar, so it is its own decision rather than a rider
    /// on this one. Only `high >= low` is checked, which is the ordering the
    /// stored table also asserts.
    pub fn validated(self) -> Result<Self> {
        for (field, value) in [
            ("low", self.low),
            ("high", self.high),
            ("open", self.open),
            ("close", self.close),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(anyhow!(
                    "{field} price {value} is not a positive finite number"
                ));
            }
        }
        // Reached only once both are finite, so this is an ordinary comparison
        // rather than one a `NaN` could silently pass by making it false.
        if self.high < self.low {
            return Err(anyhow!(
                "high {} is below low {}, so the bar was assembled or \
                 transformed wrongly",
                self.high,
                self.low
            ));
        }
        Ok(self)
    }
}

/// One batched reading: the venue's own symbol key → USD price. The key type
/// is the venue's, not ours — CoinGecko slugs are strings, CoinMarketCap ids
/// are numeric — because translating them here would just move the mapping
/// into the adapter and hide it from the caller that owns the roster.
pub type Quotes<K> = HashMap<K, f64>;

/// How many requests a floor of `interval` permits per `window`, for the
/// per-venue budget assertions each adapter's tests make.
///
/// This exists so a venue's documented limit is checked as *arithmetic* rather
/// than restated as a constant: a test that asserts `MIN_REQUEST_INTERVAL == 8s`
/// only proves the number was not edited, where one asserting it yields ≤ 8
/// requests a minute proves it still satisfies the tier it was chosen for. The
/// point is to catch a floor lowered without re-checking the venue — a one-time
/// live measurement cannot, since it decays the moment either side changes.
#[cfg(test)]
pub(crate) fn requests_per_window(
    interval: std::time::Duration,
    window: std::time::Duration,
) -> f64 {
    window.as_secs_f64() / interval.as_secs_f64()
}

#[cfg(test)]
mod candle_guard_tests {
    use super::Candle;

    /// A well-formed bar, for these tests to perturb one field of.
    fn well_formed_candle() -> Candle {
        Candle {
            bucket_start: 1_786_668_660,
            low: 1.360_00,
            high: 1.380_00,
            open: 1.370_00,
            close: 1.375_00,
            volume: 12.0,
        }
    }

    #[test]
    fn accepts_a_well_formed_bar_unchanged() {
        let bar = well_formed_candle();
        assert_eq!(
            bar.clone()
                .validated()
                .expect("a well-formed bar is storable"),
            bar,
            "the guard must return the bar it was given, not a normalized one"
        );
    }

    /// Place `value` in the named price field of an otherwise sound bar.
    fn with_price(field: &str, value: f64) -> Candle {
        let mut candle = well_formed_candle();
        match field {
            "low" => candle.low = value,
            "high" => candle.high = value,
            "open" => candle.open = value,
            "close" => candle.close = value,
            other => panic!("{other} is not a price field"),
        }
        candle
    }

    /// Every price field is checked, not just the first — the guard is a loop,
    /// and the regression worth catching is one that narrows to `low` while
    /// still passing a test that only perturbs `low`.
    ///
    /// The value set is the one a bare float parse admits. `"NaN"` and `"inf"`
    /// both parse successfully in Rust, so a venue sentinel arrives as an
    /// ordinary `f64`; and a positivity test *alone* would let `NaN` through,
    /// because `NaN <= 0.0` is false like every other `NaN` comparison. That is
    /// the Rust-side mirror of the Postgres trap, where the same value passes
    /// for the opposite reason (`NaN > 0` is true there).
    #[test]
    fn rejects_a_bad_value_in_any_of_the_four_price_fields() {
        for field in ["low", "high", "open", "close"] {
            for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.0, -1.0] {
                let err = with_price(field, value)
                    .validated()
                    .err()
                    .unwrap_or_else(|| panic!("{field} = {value} was accepted"))
                    .to_string();
                // Both the field name *and* the value-arm wording, because the
                // ordering arm's message also contains the word "high" — so a
                // bare field-name assertion would still pass for a `high` of
                // `0.0` or a negative if the value loop were deleted.
                assert!(
                    err.contains(field) && err.contains("not a positive finite number"),
                    "a bad {field} must be refused by the VALUE arm, naming that \
                     field — the name is the whole diagnostic a drop carries, and \
                     the wording is what distinguishes the two arms: {err}"
                );
            }
        }
    }

    /// An inverted bar is what a direction-flip bug produces: inverting a bar
    /// has to swap high and low, since `x -> 1/x` reverses their order, so
    /// forgetting the swap yields exactly this.
    #[test]
    fn rejects_a_bar_whose_high_is_below_its_low() {
        let mut candle = well_formed_candle();
        candle.high = candle.low - 0.01;
        let err = candle
            .validated()
            .expect_err("an inverted bar is not storable");
        assert!(
            err.to_string().contains("below low"),
            "the ordering failure must be named as such rather than as a bad \
             value, since the two mean different bugs: {err}"
        );
    }

    /// A flat bucket is legitimate — nothing traded away from one price — so the
    /// ordering check is strict `<`, matching the stored constraint's `>=`.
    /// Tightening either to reject equality would discard real quiet buckets,
    /// which is most of an FX series overnight.
    #[test]
    fn accepts_a_flat_bucket_where_high_equals_low() {
        let flat = 1.370_00;
        let candle = Candle {
            low: flat,
            high: flat,
            open: flat,
            close: flat,
            ..well_formed_candle()
        };
        assert!(
            candle.validated().is_ok(),
            "a bucket that never moved is real data, not a malformed bar"
        );
    }

    /// Volume is deliberately unconstrained, matching the column: two wired
    /// sources publish none at all and write `0.0`, so zero is routine rather
    /// than missing.
    #[test]
    fn accepts_a_bar_with_zero_volume() {
        let candle = Candle {
            volume: 0.0,
            ..well_formed_candle()
        };
        assert!(candle.validated().is_ok());
    }
}
