//! The FX-stablecoin liquidity survey app (`docs/fx-survey.md`) — the first
//! consumer of the shared `feeds` ingestion framework (`docs/data-feeds.md`),
//! and so the end-to-end proof of its source → store-sink path.
//!
//! This crate owns the survey's storage schema and its data sources; the
//! framework owns the drive loop, cursor persistence, and fan-out. The gate's
//! first feed is the [`coinbase`] EURC/USDC reference price (`fx-survey.md`
//! §4); the [`store`] module maps its records onto the `cex_prices` table and
//! provisions the schema. Two binaries drive it — `fx-survey-migrate` (run
//! once) and `fx-survey-coinbase` (the long-lived feed).

pub mod coinbase;
pub mod config;
pub mod store;
