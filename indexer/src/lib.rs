//! Prototype event indexer for the Dropset eCLOB. See `docs/indexer.md`.
//!
//! Pipeline: `ingest` (poll the cluster) → `decode` (the shared
//! `dropset_sdk::events` walk) → `store` (raw, idempotent on the event PK)
//! → `aggregate` (watermarked legs→takes + market rollups) → `api` (`/v1`).

pub mod aggregate;
pub mod api;
pub mod config;
pub mod decode;
pub mod ingest;
pub mod model;
pub mod store;
