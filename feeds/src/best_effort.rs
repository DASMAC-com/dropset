//! A sink wrapper that absorbs its inner sink's failures.

use crate::record::Batch;
use crate::sink::Sink;
use anyhow::Result;
use async_trait::async_trait;

/// After the first failure, report once every this many consecutive ones. A
/// sink that is down stays down for many batches, and one line per batch turns
/// an outage into a log flood; the first line and the recovery line are the two
/// that carry information.
const REPORT_EVERY: u64 = 50;

/// Wrap a sink so a failed batch is **dropped and logged** instead of
/// propagating.
///
/// The runner's contract is that a sink error propagates out and the process
/// crashes, to be resumed from the store cursor — which is right for the
/// warehouse path, where the records are the product. It is wrong for a
/// *telemetry* path, where the records describe a process that is still
/// running: there, a database outage taking down the sink would take the
/// runner with it, and telemetry would then stay dead for the rest of the
/// process's life even after the database came back. Since the point of the
/// telemetry is to observe the process, losing it permanently at the first
/// blip is the worst available outcome.
///
/// **This converts at-least-once delivery into at-most-once.** A batch that
/// fails is gone; there is no retry and no cursor to resume from. That is only
/// acceptable when the records are *samples of current state*, where the next
/// one supersedes what was lost — never for a ledger. So this belongs on a
/// telemetry or metrics sink and must not be wrapped around the warehouse
/// store sink, whose whole guarantee it would quietly remove.
///
/// A wrapped [`crate::StoreSink`] keeps one useful property: because the drop
/// happens after the inner sink's transaction has already failed and rolled
/// back, a dropped batch leaves no partial rows.
///
/// **One failure mode this would swallow is worse than at-most-once, and it
/// is the sharpest reason for the prohibition above.** [`crate::StoreSink`]
/// saves the feed cursor *after* its transaction commits, so a batch whose
/// commit succeeds and whose cursor save then fails returns `Err` — with the
/// rows already durable. Wrapping that turns a crash-and-resume (which heals
/// the cursor) into a silent, permanent cursor stall: the rows are in, the
/// position never advances, and the feed re-reads the same window forever.
/// That path is unreachable for a telemetry sink — a live [`crate::Source`]
/// yields batches with no cursor, so the save never runs — but it is exactly
/// what would happen on the warehouse path, where the only thing preventing
/// it is that nobody wraps that sink.
pub struct BestEffortSink<S> {
    inner: S,
    label: String,
    /// Consecutive failures; reset by the next success, so the recovery line
    /// can report how many batches were lost.
    consecutive: u64,
    /// Batches dropped over this sink's whole life, never reset.
    ///
    /// Deliberately separate from `consecutive` rather than derived from it,
    /// because the two answer different questions and only one of them
    /// survives a recovery. `consecutive` is "is it failing *now*", and a
    /// single success erases it — so a sink that drops one batch an hour,
    /// every hour, reports zero forever while losing a batch an hour. This is
    /// the total, which is what "how much have we lost" needs.
    dropped_batches: u64,
    /// Records inside those batches. Kept beside the batch count because a
    /// batch is a variable amount of data — twenty dropped heartbeat batches
    /// and one dropped backfill batch are very different losses, and the
    /// batch count alone cannot tell them apart.
    dropped_records: u64,
}

impl<S> BestEffortSink<S> {
    /// Wrap `inner`. `label` names the sink in the log lines — it is what an
    /// operator reads when telemetry goes quiet, so name the destination
    /// ("maker telemetry"), not the type.
    pub fn new(label: impl Into<String>, inner: S) -> Self {
        Self {
            inner,
            label: label.into(),
            consecutive: 0,
            dropped_batches: 0,
            dropped_records: 0,
        }
    }

    /// Batches dropped since construction, and the records they contained.
    ///
    /// Records-in-dropped-batches is an UPPER bound on records lost, not a
    /// count of them: a non-atomic inner sink may have persisted some of a
    /// batch before failing, and this wrapper cannot see how far it got.
    ///
    /// **This is the only record that anything was lost.** The wrapper's whole
    /// job is to return `Ok` when the inner sink fails, so the runner sees a
    /// success and `feed_health` records one: a sink dropping every batch is
    /// indistinguishable, from outside, from one delivering every batch. The
    /// logs carry the first failure and every fiftieth after it, which is
    /// right for noticing an outage and useless for quantifying one.
    ///
    /// Counts are per-process and reset on restart, so a difference between
    /// two readings is meaningful and an absolute value is only meaningful
    /// beside an uptime.
    pub fn dropped(&self) -> (u64, u64) {
        (self.dropped_batches, self.dropped_records)
    }
}

#[async_trait]
impl<R, S> Sink<R> for BestEffortSink<S>
where
    R: Send + Sync,
    S: Sink<R> + Send,
{
    async fn handle(&mut self, batch: &Batch<R>) -> Result<()> {
        match self.inner.handle(batch).await {
            Ok(()) => {
                if self.consecutive > 0 {
                    tracing::info!(
                        sink = %self.label,
                        lost_batches = self.consecutive,
                        "sink recovered"
                    );
                    self.consecutive = 0;
                }
            }
            Err(e) => {
                self.consecutive += 1;
                // Counted before the reporting decision below, not inside it:
                // only one line in fifty is emitted during an outage, so
                // counting where the logging happens would record one drop in
                // fifty, and would do it silently.
                self.dropped_batches += 1;
                self.dropped_records += batch.len() as u64;
                // The first failure and every REPORT_EVERY-th after it. An
                // outage is one event, not one per batch.
                if self.consecutive == 1 || self.consecutive.is_multiple_of(REPORT_EVERY) {
                    // Error as a field, not interpolated into the message:
                    // this crate's other failure logs do the same, and a log
                    // backend can then group these lines by message.
                    tracing::warn!(
                        sink = %self.label,
                        consecutive = self.consecutive,
                        records = batch.len(),
                        error = %format_args!("{e:#}"),
                        "dropping a batch"
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// A sink that fails while `failing` is set, counting the batches it was
    /// handed either way.
    struct Flaky {
        failing: Arc<AtomicBool>,
        seen: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Sink<u64> for Flaky {
        async fn handle(&mut self, batch: &Batch<u64>) -> Result<()> {
            self.seen.fetch_add(batch.len(), Ordering::SeqCst);
            if self.failing.load(Ordering::SeqCst) {
                anyhow::bail!("the database is unreachable");
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_failing_inner_sink_does_not_fail_the_runner() {
        let failing = Arc::new(AtomicBool::new(true));
        let seen = Arc::new(AtomicUsize::new(0));
        let mut sink = BestEffortSink::new(
            "test telemetry",
            Flaky {
                failing: failing.clone(),
                seen: seen.clone(),
            },
        );

        // The property the whole type exists for: the runner sees `Ok` and
        // keeps driving, so telemetry resumes when the sink recovers rather
        // than staying dead for the life of the process.
        for _ in 0..3 {
            sink.handle(&Batch::new(vec![1, 2])).await.unwrap();
        }
        assert_eq!(sink.consecutive, 3);

        failing.store(false, Ordering::SeqCst);
        sink.handle(&Batch::new(vec![3])).await.unwrap();
        assert_eq!(sink.consecutive, 0, "a success resets the run");
        assert_eq!(seen.load(Ordering::SeqCst), 7);
    }

    #[tokio::test]
    async fn a_healthy_inner_sink_is_passed_through_untouched() {
        let seen = Arc::new(AtomicUsize::new(0));
        let mut sink = BestEffortSink::new(
            "test telemetry",
            Flaky {
                failing: Arc::new(AtomicBool::new(false)),
                seen: seen.clone(),
            },
        );

        sink.handle(&Batch::new(vec![1, 2, 3])).await.unwrap();
        assert_eq!(seen.load(Ordering::SeqCst), 3);
        assert_eq!(sink.consecutive, 0);
        assert_eq!(sink.dropped(), (0, 0), "nothing failed, so nothing is lost");
    }

    #[tokio::test]
    async fn the_drop_total_survives_a_recovery() {
        // THE POINT OF THE COUNTER. `consecutive` is reset by any success, so
        // a sink that fails intermittently reports zero between blips while
        // steadily losing data. The total is what says how much is gone.
        let failing = Arc::new(AtomicBool::new(true));
        let seen = Arc::new(AtomicUsize::new(0));
        let mut sink = BestEffortSink::new(
            "test telemetry",
            Flaky {
                failing: failing.clone(),
                seen: seen.clone(),
            },
        );

        sink.handle(&Batch::new(vec![1, 2])).await.unwrap();
        sink.handle(&Batch::new(vec![3, 4, 5])).await.unwrap();
        assert_eq!(sink.dropped(), (2, 5));

        // Recover: `consecutive` goes to zero, the total must NOT.
        failing.store(false, Ordering::SeqCst);
        sink.handle(&Batch::new(vec![6])).await.unwrap();
        assert_eq!(sink.consecutive, 0);
        assert_eq!(
            sink.dropped(),
            (2, 5),
            "a success must not erase what was already lost"
        );

        // Fail again: the total accumulates across the outages rather than
        // restarting with each one.
        failing.store(true, Ordering::SeqCst);
        sink.handle(&Batch::new(vec![7, 8])).await.unwrap();
        assert_eq!(sink.dropped(), (3, 7));
    }

    #[tokio::test]
    async fn every_dropped_batch_is_counted_not_just_the_reported_ones() {
        // Only the first failure and every REPORT_EVERY-th after it is logged.
        // Counting where the logging happens would record one drop in fifty,
        // so this drives more than one reporting interval and checks the count
        // against the batches actually handed over.
        let batches = (REPORT_EVERY * 2 + 7) as usize;
        let mut sink = BestEffortSink::new(
            "test telemetry",
            Flaky {
                failing: Arc::new(AtomicBool::new(true)),
                seen: Arc::new(AtomicUsize::new(0)),
            },
        );

        for _ in 0..batches {
            sink.handle(&Batch::new(vec![1, 2, 3])).await.unwrap();
        }

        assert_eq!(sink.dropped(), (batches as u64, batches as u64 * 3));
    }
}
