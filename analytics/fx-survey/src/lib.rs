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
//! The first feed is the shared Coinbase EURC/USDC reference price
//! (`data-feeds.md` §9); the [`store`] module maps its candles onto the
//! `cex_prices` table. One binary drives it — `fx-survey-coinbase`, the
//! long-lived feed — and it asserts the shared schema at startup instead of
//! provisioning one (`data-feeds.md` §8).

pub mod config;
pub mod store;
