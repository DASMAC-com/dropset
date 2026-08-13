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
//!   These implement [`BatchQuotes`]; [`coingecko`], [`coinmarketcap`],
//!   [`frankfurter`], and [`kraken`] are the four today. Batching is the
//!   per-venue budget's main lever (§10): one poll for N markets, not N polls.
//! - **Per-product venues** — the endpoint is keyed by a single product, so
//!   batching is not on offer and one source covers one product. [`coinbase`]
//!   is both cases: its candles endpoint pages its own backfill, and its
//!   ticker endpoint yields one spot price. Neither implements [`BatchQuotes`].
//! - **Batched venues richer than a price** — [`pyth`] batches like the first
//!   group but yields a confidence half-width and a publish time alongside each
//!   rate, so it cannot ride [`Quotes`]' bare `f64` and does not implement
//!   [`BatchQuotes`] either. That extra payload is precisely what makes it the
//!   FX anchor's *primary* tier rather than another fallback.
//!
//! Each adapter splits its decode out into free `parse_*` functions, which need
//! no network: they are unit tested against captured responses, so a venue's
//! JSON shape stays covered without anything reaching the venue itself. Only
//! the transport half needs a network, and nothing here tests that.
//!
//! **Credentials arrive by injection, never by an environment read in here.**
//! A keyed adapter takes its key as a constructor argument
//! ([`coinmarketcap::CmcSource::new`]) so the caller decides where the secret
//! came from — a process environment today, a secrets provider later — and no
//! adapter has to change when that answer does.

use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::Hash;

// Each venue rides its own transport's gate, not the module's — the contract
// below is transport-free, so a future streaming venue lands here too.
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
pub mod pyth;

#[cfg(feature = "http")]
pub use coinbase::{Candle, CoinbaseCandles, CoinbaseTicker};
#[cfg(feature = "http")]
pub use coingecko::CoinGeckoSource;
#[cfg(feature = "http")]
pub use coinmarketcap::CmcSource;
#[cfg(feature = "http")]
pub use frankfurter::FrankfurterSource;
#[cfg(feature = "http")]
pub use kraken::KrakenSource;
#[cfg(feature = "http")]
pub use pyth::{FxQuote, PythFeed, PythHermesSource};

/// One batched reading: the venue's own symbol key → USD price. The key type
/// is the venue's, not ours — CoinGecko slugs are strings, CoinMarketCap ids
/// are numeric — because translating them here would just move the mapping
/// into the adapter and hide it from the caller that owns the roster.
pub type Quotes<K> = HashMap<K, f64>;

/// A venue whose endpoint quotes **many symbols in one request**.
///
/// The adapter is built with its full symbol set and fetches all of them per
/// poll, which is what lets one process serve a whole roster inside a keyless
/// tier's budget (docs/data-feeds.md §10). Implementors also implement
/// [`crate::Source`], whose `next` is just this poll wrapped in a batch — so
/// the same adapter drives the runner *and* answers a caller that wants one
/// synchronous reading (a `--dry-run` credentials check) with no runner at all.
#[async_trait]
pub trait BatchQuotes: Send + Sync {
    /// The venue's symbol key.
    type Symbol: Eq + Hash + Send + Sync;

    /// Fetch every symbol this adapter was built with, in one request.
    /// Symbols the venue does not quote are **omitted** rather than erroring:
    /// a roster with one unlisted token still prices the rest.
    async fn poll(&self) -> Result<Quotes<Self::Symbol>>;
}
