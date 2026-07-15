//! The subscribe / streaming seam (`stream` feature): the channel bridge a
//! push transport funnels into the async [`Source`] model.

use crate::record::Batch;
use crate::source::Source;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// The most records a single [`ChannelSource::next`] drains before yielding, so
/// a burst becomes a batch rather than one call each.
const DRAIN_CAP: usize = 256;

/// A subscribe source: bridges records pushed from a background transport (a
/// WebSocket / `logsSubscribe` / geyser client) into the async [`Source`]
/// model. This is the reusable seam every push source funnels through; the
/// concrete socket — its reconnect policy, filter, and message schema — lives
/// with its first consumer (docs/data-feeds.md §4, §7), which spawns a task
/// that pushes into the returned [`mpsc::Sender`].
///
/// `next` awaits the next record, then drains any already-queued ones into the
/// same batch (up to [`DRAIN_CAP`]), and returns it with no cursor — a live
/// stream has nothing to resume. When every sender is dropped it yields empty
/// caught-up batches, so the runner idles rather than spins.
pub struct ChannelSource<R> {
    name: String,
    rx: mpsc::Receiver<R>,
}

impl<R: Send + 'static> ChannelSource<R> {
    /// Build a source and the sender a transport task pushes into. `buffer`
    /// bounds the channel between the transport and the runner.
    pub fn new(name: impl Into<String>, buffer: usize) -> (Self, mpsc::Sender<R>) {
        let (tx, rx) = mpsc::channel(buffer);
        (
            Self {
                name: name.into(),
                rx,
            },
            tx,
        )
    }
}

#[async_trait]
impl<R: Send + 'static> Source for ChannelSource<R> {
    type Record = R;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<R>> {
        let first = match self.rx.recv().await {
            Some(record) => record,
            // All senders dropped — the transport stopped. Idle, don't error.
            None => return Ok(Batch::new(vec![])),
        };
        let mut records = vec![first];
        while records.len() < DRAIN_CAP {
            match self.rx.try_recv() {
                Ok(record) => records.push(record),
                Err(_) => break,
            }
        }
        Ok(Batch::new(records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drains_queued_records_into_one_batch() {
        let (mut source, tx) = ChannelSource::<u64>::new("push", 16);
        tx.send(1).await.unwrap();
        tx.send(2).await.unwrap();
        tx.send(3).await.unwrap();

        let batch = source.next().await.unwrap();
        assert_eq!(batch.records, vec![1, 2, 3]);
        assert!(batch.caught_up);
        assert!(batch.cursor.is_none());
    }

    #[tokio::test]
    async fn yields_empty_once_the_transport_stops() {
        let (mut source, tx) = ChannelSource::<u64>::new("push", 16);
        tx.send(9).await.unwrap();
        drop(tx);

        assert_eq!(source.next().await.unwrap().records, vec![9]);
        assert!(source.next().await.unwrap().is_empty());
    }
}
