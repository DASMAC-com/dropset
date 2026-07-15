//! The `Sink` trait — where records go.

use crate::record::Batch;
use anyhow::Result;
use async_trait::async_trait;

/// A destination for a source's records. A source is fanned out to one or
/// more sinks; the runner calls [`Sink::handle`] on each with every batch, in
/// order. An error propagates out of the runner (let it crash and resume from
/// the store cursor); a best-effort sink such as [`crate::ForwardSink`]
/// swallows its own non-fatal drops and returns `Ok`.
#[async_trait]
pub trait Sink<R>: Send {
    async fn handle(&mut self, batch: &Batch<R>) -> Result<()>;
}
