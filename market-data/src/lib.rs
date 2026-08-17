//! The market-data collection app (`docs/data-feeds.md` §7) — the first
//! consumer of the shared `feeds` ingestion framework, and so the end-to-end
//! proof of its source → store-sink path.
//!
//! This crate owns the collector's **deployment shape** — its configuration and
//! its record → row mapping. The venue adapters it polls are not its own: they
//! live in `dropset_feeds::venues`, written once and shared with the bots
//! (`data-feeds.md` §4), so this crate names a source rather than implementing
//! one. The framework owns the drive loop, cursor persistence, and fan-out, and
//! `dropset-db-schema` owns every table either of them touches.
//!
//! The first feed was the shared Coinbase EURC/USDC reference price
//! (`data-feeds.md` §9); the [`store`] module maps every candle source onto the
//! `cex_prices` table. Four binaries drive the collectors today — `coinbase`
//! for the crypto reference price, and `oanda` / `twelvedata` / `alphavantage`
//! for the free-tier FX roster — and each asserts the shared schema at startup
//! instead of provisioning one (`data-feeds.md` §8).
//!
//! The FX collectors share the [`fx`] module: their configuration, the single
//! place a credential is resolved, and the canonical ↔ venue symbol mapping.
//! That last part is load-bearing rather than cosmetic — the three FX vendors
//! spell one currency pair three different ways, so the **stored** `product_id`
//! is canonical (`AUD-USD`) and each adapter is handed the spelling it wants.
//! Storing venue-native symbols would put one pair under three keys and make
//! the cross-source comparison these feeds exist for impossible.

pub mod config;
pub mod fx;
pub mod store;
