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
//! - **Batched venues richer than a price** — [`pyth`] batches like the first
//!   group but yields a confidence half-width and a publish time alongside each
//!   rate, so it cannot ride [`Quotes`]' bare `f64`. That extra payload is
//!   precisely what makes it the FX anchor's *primary* tier rather than another
//!   fallback.
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
pub use frankfurter::FrankfurterSource;
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
