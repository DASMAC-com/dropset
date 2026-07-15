//! The forward (live) sink: an in-process broadcast channel.

use crate::record::Batch;
use crate::sink::Sink;
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::broadcast;

/// An in-process live sink: forwards each record onto a bounded broadcast
/// channel with no persistence, for a co-located consumer (a bot) that reads
/// the tail. Latency, not durability, is the point (docs/data-feeds.md §3).
///
/// The channel is bounded and **drops to the latest** for a slow consumer: a
/// lagging receiver observes [`broadcast::error::RecvError::Lagged`] and is
/// fast-forwarded to the newest record, so one slow bot never stalls a source
/// shared with a store sink (docs/data-feeds.md §7). A bot wants freshest, not
/// complete.
pub struct ForwardSink<R> {
    tx: broadcast::Sender<R>,
}

/// Build a [`ForwardSink`] and its first receiver. `capacity` bounds the
/// channel; further receivers come from [`ForwardSink::subscribe`].
pub fn forward_channel<R: Clone>(capacity: usize) -> (ForwardSink<R>, broadcast::Receiver<R>) {
    let (tx, rx) = broadcast::channel(capacity);
    (ForwardSink { tx }, rx)
}

impl<R: Clone> ForwardSink<R> {
    /// A new receiver on the same channel — each sees every record sent after
    /// it subscribes.
    pub fn subscribe(&self) -> broadcast::Receiver<R> {
        self.tx.subscribe()
    }

    /// Live receiver count — a source can pause when nobody is listening.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[async_trait]
impl<R: Clone + Send + Sync> Sink<R> for ForwardSink<R> {
    async fn handle(&mut self, batch: &Batch<R>) -> Result<()> {
        for record in &batch.records {
            // `send` errors only when there are no receivers; that is not a
            // failure for a best-effort live sink — drop and keep going.
            let _ = self.tx.send(record.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn forwards_every_record_to_a_live_receiver() {
        let (mut sink, mut rx) = forward_channel::<u64>(16);
        sink.handle(&Batch::new(vec![1, 2, 3])).await.unwrap();

        let mut got = Vec::new();
        while let Ok(v) = rx.try_recv() {
            got.push(v);
        }
        assert_eq!(got, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn drops_to_latest_for_a_slow_consumer() {
        // Capacity 2, but four records sent before the receiver reads: the
        // receiver must observe a lag rather than back-pressure the sender.
        let (mut sink, mut rx) = forward_channel::<u64>(2);
        sink.handle(&Batch::new(vec![1, 2, 3, 4])).await.unwrap();

        match rx.recv().await {
            Err(broadcast::error::RecvError::Lagged(_)) => {}
            other => panic!("expected a lag for the slow consumer, got {other:?}"),
        }
        // After the lag it resumes at the newest retained records.
        assert_eq!(rx.recv().await.unwrap(), 3);
        assert_eq!(rx.recv().await.unwrap(), 4);
    }
}
