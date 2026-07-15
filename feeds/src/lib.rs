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
//! Source adapters are feature-gated ([`HttpClient`] behind `http`,
//! [`RpcPollSource`] behind `rpc`, [`ChannelSource`] behind `stream`) so a
//! consumer compiles only the transport it uses. The design is
//! `docs/data-feeds.md`.

mod cursor;
mod forward;
mod record;
mod runner;
mod sink;
mod source;

pub use cursor::{Cursor, CursorStore};
pub use forward::{forward_channel, ForwardSink};
pub use record::Batch;
pub use runner::{run, run_until, RunConfig};
pub use sink::Sink;
pub use source::Source;

#[cfg(feature = "store")]
mod store;
#[cfg(feature = "store")]
pub use store::{connect, PgCursorStore, StoreSink, StoreWriter};

#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::HttpClient;

#[cfg(feature = "rpc")]
mod rpc;
#[cfg(feature = "rpc")]
pub use rpc::{RawTx, RpcPollSource};

#[cfg(feature = "stream")]
mod stream;
#[cfg(feature = "stream")]
pub use stream::ChannelSource;
