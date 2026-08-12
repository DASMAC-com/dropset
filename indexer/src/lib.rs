//! Prototype event indexer for the Dropset eCLOB. See `docs/indexer.md`.
//!
//! Pipeline: the shared framework's RPC poll source (`dropset_feeds`) polls
//! the cluster → `decode` (the shared `dropset_sdk::events` walk) → `store`
//! (raw, idempotent on the event PK, behind the framework's store sink) →
//! `aggregate` (watermarked legs→takes + market rollups) → `api` (`/v1`).
//!
//! The transport, the batch transaction, and the resume cursor come from
//! `dropset_feeds` (docs/data-feeds.md §6); what stays here is everything
//! Dropset-specific — the event decode, the row writers, the aggregator, and
//! the `/v1` surface.

pub mod aggregate;
pub mod api;
pub mod config;
pub mod decode;
pub mod model;
pub mod store;
