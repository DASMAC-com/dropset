//! The Dropset **feeds** ingestion framework — source → records → sinks.
//!
//! A [`Source`] fetches or subscribes to a data source and yields typed
//! records; the [`run`] runner fans each [`Batch`] to one or more [`Sink`]s.
//! Two sink kinds sit on a durability-vs-latency axis, independent of how the
//! source drives:
//!
//! - a **store sink** ([`StoreSink`], `store` feature) — idempotent Postgres
//!   persistence behind a resumable JSONB [`Cursor`] (the warehouse path);
//! - a **forward sink** ([`ForwardSink`]) — an in-process broadcast channel a
//!   co-located consumer reads with minimal latency and no persistence (the
//!   bot path).
//!
//! Transports are feature-gated ([`HttpClient`] behind `http`,
//! [`RpcPollSource`] behind `rpc`, [`ChannelSource`] behind `stream`) so a
//! consumer compiles only the transport it uses. The concrete venue adapters
//! built on them live in [`venues`] — written once here rather than stranded
//! in whichever app needed one first, so a collector and a bot share the same
//! source and differ only in which sink they wire it to. The design is
//! `docs/data-feeds.md`.
//!
//! Two facilities cut across every poll source: [`Backfill`] keeps a
//! newest-first transport's backlog draining oldest-first so a resume cursor
//! never skips the middle of it, and [`FeedMetrics`] is the seam the runner
//! reports batches and errors through, so a deployed feed is observable
//! without per-feed wiring.

mod backfill;
mod cursor;
mod forward;
mod record;
mod runner;
mod sink;
mod source;
mod time;

pub use backfill::{Backfill, Step};
pub use cursor::{Cursor, CursorStore};
pub use forward::{forward_channel, ForwardSink};
pub use record::Batch;
pub use runner::{
    run, run_until, run_until_with_metrics, run_with_metrics, BatchStats, FeedMetrics, NoopMetrics,
    RunConfig,
};
pub use sink::Sink;
pub use source::Source;
pub use time::now_secs;

#[cfg(feature = "store")]
mod store;
#[cfg(feature = "store")]
pub use store::{connect, PgCursorStore, StoreSink, StoreWriter};

#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::HttpClient;

// Not gated, on purpose: the `BatchQuotes` contract needs no transport, so a
// streaming venue can implement it without pulling in `http`. Each venue
// submodule carries its own transport's gate instead — every adapter shipped
// today polls REST, so today they all ride `http`.
pub mod venues;

#[cfg(feature = "rpc")]
mod rpc;
#[cfg(feature = "rpc")]
pub use rpc::{RawTx, RpcPollSource};

#[cfg(feature = "stream")]
mod stream;
#[cfg(feature = "stream")]
pub use stream::ChannelSource;
