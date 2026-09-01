//! The drop-damping a telemetry reporter offers records through.
//!
//! Both of this crate's reporters — [`crate::HealthReporter`] on the runner's
//! metrics seam, [`crate::LivenessReporter`] on a push transport's own thread —
//! hand records to a consumer's bounded channel from a context that **must not
//! block**: one is called inline on a drive loop, the other sits on the thread
//! a socket's reconnect depends on. So both `try_send` and return, and both
//! need the same answer to "what is said when the channel is full or the drain
//! is gone" — which is a surprising amount of care for what looks like one log
//! line, and was written once for health before there was a second caller.
//!
//! What it deliberately does **not** decide is whether a dropped record is
//! recoverable, because that differs between the two and is the caller's to
//! know. A health update is level-triggered: the next turn restates the same
//! liveness, so a drop costs resolution and nothing else. A liveness
//! transition is edge-triggered, so a dropped one is a persistently wrong row.
//! [`DampedSender::offer`] therefore reports whether the record was accepted
//! and leaves the response to the caller — [`crate::LivenessReporter`] retains
//! and retries; [`crate::HealthReporter`] ignores it.

use crate::time::now_secs;
use tokio::sync::mpsc;

/// How many consecutive drops pass before the sender says so. A full channel
/// means the drain is behind, which is worth one line rather than one per
/// dropped record — and rather than silence, which would make a consumer's
/// table that has quietly stopped advancing look like a healthy idle feed.
///
/// Counted as a **consecutive** run and reset on the next success, so the
/// number in the log is the length of the current outage rather than a
/// lifetime total of unrelated blips.
const DROP_REPORT_EVERY: u64 = 100;

/// The shortest gap between two dropped-record warnings, in seconds.
///
/// The count-based damping above handles a *sustained* outage, but not a
/// flapping one: the counter resets on every success, so a drain hovering at
/// the edge of its write timeout scores `dropped == 1` on every turn and earns
/// the first-of-a-run line every time — a warning plus a recovery line per
/// turn, forever, which is the volume the damping exists to prevent. A
/// wall-clock floor bounds the flapping case without going silent on it.
const MIN_WARN_INTERVAL_SECS: i64 = 60;

/// A bounded channel a reporter offers records into, dropping rather than
/// waiting, with the damping that keeps a sustained or flapping outage to a
/// readable number of log lines.
pub(crate) struct DampedSender<R> {
    tx: mpsc::Sender<R>,
    /// What these records describe, for the log lines — `"health"`,
    /// `"liveness"`. A field rather than baked into each message so an
    /// operator filtering the two apart can, while the reasoning above stays
    /// in one place.
    kind: &'static str,
    /// Consecutive drops, reset on the next success.
    dropped: u64,
    /// Whether the permanent-closure line has been said.
    ///
    /// Separate from `dropped` on purpose. Latching that line on
    /// `dropped == 0` looks equivalent and is not: a full channel increments
    /// the same counter, so a drain that first falls *behind* and then *dies*
    /// — the overwhelmingly likely order, since a drain slow enough to fill
    /// the channel is the one most likely to fail next — arrives at the
    /// `Closed` arm with a non-zero count and says nothing at all. The
    /// operator's last line then reads "the drain is behind", meaning
    /// transient backlog, while the true state is permanent loss.
    closed_reported: bool,
    /// Epoch second of the last drop warning, for [`MIN_WARN_INTERVAL_SECS`].
    last_warn_at: i64,
    /// Whether the current run of drops was actually warned about, so the
    /// recovery line only speaks when there is something to recover *from*
    /// that the operator was told about.
    warned: bool,
}

impl<R> DampedSender<R> {
    /// Send onto `tx`, labelling this sender's log lines `kind`. Bound the
    /// channel when building it: this drops rather than waits, which is the
    /// property that keeps a slow drain off the caller's hot path.
    pub(crate) fn new(tx: mpsc::Sender<R>, kind: &'static str) -> Self {
        Self {
            tx,
            kind,
            dropped: 0,
            closed_reported: false,
            last_warn_at: 0,
            warned: false,
        }
    }

    /// Offer one record for `feed`, dropping it if the channel is full or
    /// closed, and report whether it was accepted.
    ///
    /// Deliberately infallible: a telemetry report that could fail its caller
    /// would put the observability path in a position to take down the thing
    /// it observes. The `bool` is information, not an error — a caller whose
    /// record is level-triggered may ignore it.
    pub(crate) fn offer(&mut self, feed: &str, record: R) -> bool {
        match self.tx.try_send(record) {
            Ok(()) => {
                if self.dropped > 0 {
                    // Only if the run was actually warned about — otherwise a
                    // flapping drain earns a recovery line for an outage the
                    // operator was never told had started.
                    if self.warned {
                        tracing::info!(
                            feed = %feed,
                            kind = %self.kind,
                            recovered_after = self.dropped,
                            "feed telemetry flowing again"
                        );
                    }
                    self.dropped = 0;
                    self.warned = false;
                }
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                // Permanent: no later offer can succeed, so say it once rather
                // than one line per N forever. Latched on its own flag, not on
                // `dropped` — see the field's comment for why sharing the
                // counter silences exactly the case that most needs a line.
                // Carries the count, so a `Closed` that follows a `Full` run
                // reports the whole outage rather than just its tail.
                if !self.closed_reported {
                    self.closed_reported = true;
                    tracing::warn!(
                        feed = %feed,
                        kind = %self.kind,
                        dropped = self.dropped,
                        "the feed telemetry drain is gone — records will be \
                         dropped for the life of this process"
                    );
                }
                false
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                // The first of a run and every Nth after it — matching
                // `BestEffortSink`, whose reasoning is that the first line and
                // the recovery line are the two that carry information. The
                // counter is a *consecutive* run, reset above on success, so
                // the number in the log means what it says — and the
                // wall-clock floor keeps a flapping drain, which scores
                // `dropped == 1` every turn, from earning that line every
                // turn.
                let now = now_secs();
                let due = self.dropped == 1 || self.dropped.is_multiple_of(DROP_REPORT_EVERY);
                if due && now.saturating_sub(self.last_warn_at) >= MIN_WARN_INTERVAL_SECS {
                    self.last_warn_at = now;
                    self.warned = true;
                    tracing::warn!(
                        feed = %feed,
                        kind = %self.kind,
                        dropped = self.dropped,
                        "feed telemetry dropped — the drain is behind"
                    );
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_accepted_record_reports_true() {
        let (tx, mut rx) = mpsc::channel::<u8>(4);
        let mut sender = DampedSender::new(tx, "test");

        assert!(sender.offer("kraken", 1));
        assert_eq!(rx.try_recv().unwrap(), 1);
        assert_eq!(sender.dropped, 0);
    }

    #[tokio::test]
    async fn a_full_channel_drops_instead_of_blocking() {
        // Capacity 1, three offers: the caller's hot path must not be held up,
        // so the surplus is dropped and each call still returns.
        let (tx, mut rx) = mpsc::channel::<u8>(1);
        let mut sender = DampedSender::new(tx, "test");

        assert!(sender.offer("coingecko", 1));
        assert!(!sender.offer("coingecko", 2), "capacity was 1");
        assert!(!sender.offer("coingecko", 3));
        assert_eq!(sender.dropped, 2);

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "capacity was 1");
    }

    #[tokio::test]
    async fn a_closed_channel_is_not_an_error() {
        let (tx, rx) = mpsc::channel::<u8>(4);
        drop(rx);
        let mut sender = DampedSender::new(tx, "test");

        // A consumer that has gone away must not be able to fail its caller.
        assert!(!sender.offer("frankfurter", 1));
        assert_eq!(sender.dropped, 1);
        assert!(sender.closed_reported, "the permanent case must be said");

        // Latched: the line is said once, not once per offer.
        assert!(!sender.offer("frankfurter", 2));
        assert_eq!(sender.dropped, 2, "but the count keeps accruing");
    }

    /// A drain that falls behind and *then* dies is the likely order — the
    /// drain slow enough to fill the channel is the one most likely to fail
    /// next — and it is the case a shared latch silences: the `Closed` arm
    /// would see a non-zero drop count and say nothing, leaving "the drain is
    /// behind" (transient) as the operator's last word on permanent loss.
    #[tokio::test]
    async fn a_full_run_does_not_swallow_the_permanent_closure_line() {
        let (tx, rx) = mpsc::channel::<u8>(1);
        let mut sender = DampedSender::new(tx, "test");

        // Fall behind first: capacity 1, so the surplus drops.
        for n in 0..3 {
            sender.offer("kraken", n);
        }
        assert_eq!(sender.dropped, 2);
        assert!(!sender.closed_reported, "still merely behind");

        // Then the drain dies.
        drop(rx);
        sender.offer("kraken", 9);

        assert!(
            sender.closed_reported,
            "a preceding Full run must not suppress the permanent line"
        );
        // And the count carries the whole outage, not just its tail.
        assert_eq!(sender.dropped, 3);
    }

    /// The count-based damping resets on every success, so a flapping drain
    /// scores `dropped == 1` every turn and would earn the first-of-a-run
    /// warning every turn. The wall-clock floor is what bounds it.
    #[tokio::test]
    async fn a_flapping_drain_is_warned_about_once_not_every_turn() {
        let (tx, mut rx) = mpsc::channel::<u8>(1);
        let mut sender = DampedSender::new(tx, "test");

        // First drop of the first run: warned, and the clock is stamped.
        sender.offer("oanda", 1);
        sender.offer("oanda", 2);
        assert!(sender.warned);
        let stamped = sender.last_warn_at;

        // Drain, then flap again inside the interval.
        assert!(rx.try_recv().is_ok());
        sender.offer("oanda", 3);
        assert_eq!(sender.dropped, 0, "the success reset the run");
        assert!(!sender.warned, "and cleared the outstanding warning");

        sender.offer("oanda", 4);
        assert_eq!(sender.dropped, 1, "a fresh first-of-run drop");
        assert!(
            !sender.warned,
            "but inside MIN_WARN_INTERVAL_SECS it stays quiet"
        );
        assert_eq!(sender.last_warn_at, stamped, "and does not re-stamp");
    }
}
