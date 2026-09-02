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
//! without per-feed wiring. [`HealthReporter`] is that seam's ready-made
//! implementation: wired at spawn time it forwards every source's liveness
//! onto a channel, so a consumer gets a per-feed status row for an adapter it
//! never named.
//!
//! **A push source needs the other one.** Observability splits on how a source
//! drives, because the runner's seam can only report what the transport
//! delivered — which for a subscription is the last *record*, not the last
//! healthy socket, so a quiet market and a dead one look identical there. So a
//! poll source reports through [`HealthReporter`] and a push source reports its
//! transport state through [`LivenessReporter`], from the producer's own thread
//! rather than the drive loop. The two are separate seams on purpose, not one
//! seam with a mode: they are measured by different code, mean different
//! things, and are alerted on differently — silence is a failure for one and
//! the healthy state for the other.
//!
//! One wrapper sits across the sink axis rather than on it:
//! [`BestEffortSink`] absorbs its inner sink's failures instead of
//! propagating them, trading the runner's crash-and-resume contract for
//! survival. That trade is right for a telemetry sink — whose records describe
//! a process that is still running — and wrong for the warehouse path, so it
//! is opt-in per sink rather than a mode of the runner.
//!
//! A keyed venue's credential is resolved through [`secrets`], never read from
//! the environment by an adapter: an adapter takes its key as an argument, and
//! the app decides which store it came from.

mod backfill;
mod best_effort;
mod cursor;
mod damped;
mod forward;
mod health;
mod liveness;
mod record;
mod runner;
pub mod secrets;
mod sink;
mod source;
mod time;

pub use backfill::{Backfill, BackfillStep};
pub use best_effort::BestEffortSink;
pub use cursor::{Cursor, CursorStore};
pub use forward::{forward_channel, ForwardSink};
pub use health::{
    redact_to_origin, sanitize_error, HealthOutcome, HealthReporter, HealthUpdate, MAX_ERROR_CHARS,
};
// Not gated behind `stream`, on purpose: these are the vocabulary a push
// producer reports in, and a producer need not funnel through
// [`ChannelSource`] to have a socket worth reporting on. Gating would make a
// consumer enable a transport feature to name a state.
pub use liveness::{LinkState, LivenessReporter, LivenessUpdate};
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
pub use store::{connect, connect_lazy, PgCursorStore, StoreSink, StoreWriter};

#[cfg(feature = "http")]
mod http;
#[cfg(feature = "http")]
pub use http::HttpClient;

// Not gated, on purpose: the module's shared vocabulary (`Quotes`, `Candle`)
// needs no transport, so a streaming venue can land here without pulling in
// `http`. Each venue submodule carries its own transport's gate instead — every
// adapter shipped today polls REST, so today they all ride `http`.
pub mod venues;

#[cfg(feature = "rpc")]
mod rpc;
#[cfg(feature = "rpc")]
pub use rpc::{InnerIx, RawTx, RpcPollSource, RpcTransport};

#[cfg(feature = "stream")]
mod stream;
#[cfg(feature = "stream")]
pub use stream::ChannelSource;
