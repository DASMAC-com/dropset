//! The market-data collection app (`docs/data-feeds.md` §7) — the first
//! consumer of the shared `feeds` ingestion framework, and so the end-to-end
//! proof of its source → store-sink path.
//!
//! This crate owns the collector's data sources and its record → row mapping;
//! the framework owns the drive loop, cursor persistence, and fan-out, and
//! `dropset-db-schema` owns every table either of them touches. The first
//! feed is the [`coinbase`] EURC/USDC reference price (`data-feeds.md` §9); the
//! [`store`] module maps its records onto the `cex_prices` table. One binary
//! drives it — `fx-survey-coinbase`, the long-lived feed — and it asserts the
//! shared schema at startup instead of provisioning one (`data-feeds.md` §8).

pub mod coinbase;
pub mod config;
pub mod store;
