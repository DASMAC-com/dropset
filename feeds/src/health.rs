//! Per-feed liveness, reported generically through the runner's metrics seam.
//!
//! [`FeedMetrics`] already gives the runner a place to report every turn of
//! every source's drive loop, which is what makes liveness a *framework*
//! concern rather than a per-adapter one: a [`HealthReporter`] wired at spawn
//! time reports for whatever source it was handed, so a venue adapter added
//! later shows up in a consumer's health table having wired nothing.
//!
//! What it can report is bounded by what the seam carries, and the bound is
//! worth stating because it shapes the consumer's schema. The runner hands a
//! recorder a feed *name* and a [`BatchStats`] — never the records. So this
//! reports whether the poller is alive, when it last succeeded, and what it
//! last failed with; it cannot report what the feed *said*. Per-instrument
//! values belong to the consumer that resolved the instrument, and that is not
//! a gap to close here: a venue source (`pyth-hermes`, `kraken`) yields a map
//! of many instruments in one batch, so no single value on a per-source row
//! could be anything but an arbitrary pick.
//!
//! The channel is the whole design. [`FeedMetrics`] implementations are called
//! **inline on the drive loop and must not block**, so this only ever
//! `try_send`s and returns — a full channel drops the update rather than
//! stalling a price poll behind a database write. Dropping is the right
//! failure here because the consumer's row is *last-state-wins*: the next turn
//! reports the same liveness a moment later, so a dropped update costs
//! resolution, never correctness.

use crate::runner::{BatchStats, FeedMetrics};
use crate::time::now_secs;
use tokio::sync::mpsc;

/// The longest error text a [`HealthUpdate`] carries. A failing source can
/// produce an arbitrarily long chain (a transport error wrapping a body), and
/// this text is written to a row on every retry — so it is bounded at the
/// source rather than left for each consumer to bound.
///
/// Public because a consumer sizing a column, or sanitizing its own error
/// text with [`sanitize_error`], needs the same number — two "matched by
/// hand" constants is how they drift. Note this is a **character** count, so
/// worst-case UTF-8 is ~4× this many bytes: the destination wants `TEXT`, not
/// a `varchar(500)`.
pub const MAX_ERROR_CHARS: usize = 500;

/// How many consecutive drops pass before the reporter says so. A full channel
/// means the drain is behind, which is worth one line rather than one per
/// dropped update — and rather than silence, which would make a health table
/// that has quietly stopped advancing look like a healthy idle feed.
///
/// Counted as a **consecutive** run and reset on the next success, so the
/// number in the log is the length of the current outage rather than a
/// lifetime total of unrelated blips.
const DROP_REPORT_EVERY: u64 = 100;

/// What the most recent turn of a feed's drive loop did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HealthOutcome {
    /// A batch was fetched. Deliberately **not** "and fanned out": a sink
    /// wrapped in [`crate::BestEffortSink`] reports success to the runner
    /// even when it dropped the batch, so this says the *source* answered and
    /// nothing more. A dropped batch is visible in that wrapper's log, not
    /// here.
    Ok {
        /// Records in that batch.
        records: usize,
        /// Whether the source reported it had reached the present. Repeated
        /// `false` is the framework's only generic lag signal — see
        /// [`BatchStats`].
        caught_up: bool,
    },
    /// [`crate::Source::next`] failed and the runner is backing off, with the
    /// error rendered (cause chain included) and truncated.
    Error(String),
}

/// One observation of one feed source's liveness.
#[derive(Clone, Debug)]
pub struct HealthUpdate {
    /// The source's [`crate::Source::name`] — the key a consumer upserts on.
    pub feed: String,
    /// When this turn was observed, as an epoch second.
    pub at: i64,
    /// What the turn did.
    pub outcome: HealthOutcome,
}

/// A [`FeedMetrics`] recorder that forwards each turn's liveness onto a
/// channel, for a consumer that persists or renders it.
///
/// Generic over the consumer's own record type rather than sending
/// [`HealthUpdate`] directly, so a consumer that funnels several kinds of
/// telemetry down **one** channel — and therefore writes them in one
/// transaction, through one [`crate::StoreWriter`] — can carry health as one
/// variant of its own enum. `R: From<HealthUpdate>` is the only coupling; the
/// framework never learns the consumer's shape.
pub struct HealthReporter<R> {
    tx: mpsc::Sender<R>,
    dropped: u64,
}

impl<R> HealthReporter<R> {
    /// Report onto `tx`. Bound the channel when building it: this reporter
    /// drops rather than waits, which is the property that keeps a slow drain
    /// off the drive loop.
    pub fn new(tx: mpsc::Sender<R>) -> Self {
        Self { tx, dropped: 0 }
    }
}

impl<R: From<HealthUpdate>> HealthReporter<R> {
    /// Offer one update, dropping it if the channel is full or closed.
    ///
    /// Deliberately infallible: a health report that could fail the caller
    /// would put the telemetry path in a position to take down the feed it is
    /// reporting on.
    fn offer(&mut self, update: HealthUpdate) {
        match self.tx.try_send(R::from(update)) {
            Ok(()) => {
                if self.dropped > 0 {
                    tracing::info!(
                        recovered_after = self.dropped,
                        "feed health updates flowing again"
                    );
                    self.dropped = 0;
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Permanent: no later turn can succeed, so say it once rather
                // than one line per N turns forever.
                if self.dropped == 0 {
                    tracing::warn!(
                        "the feed health drain is gone — health updates will \
                         be dropped for the life of this process"
                    );
                }
                self.dropped = self.dropped.saturating_add(1);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                // The first of a run and every Nth after it — matching
                // `BestEffortSink`, whose reasoning is that the first line and
                // the recovery line are the two that carry information. The
                // counter is a *consecutive* run, reset above on success, so
                // the number in the log means what it says.
                if self.dropped == 1 || self.dropped.is_multiple_of(DROP_REPORT_EVERY) {
                    tracing::warn!(
                        dropped = self.dropped,
                        "feed health updates dropped — the telemetry drain is \
                         behind"
                    );
                }
            }
        }
    }
}

impl<R: From<HealthUpdate> + Send> FeedMetrics for HealthReporter<R> {
    fn on_batch(&mut self, feed: &str, stats: &BatchStats) {
        self.offer(HealthUpdate {
            feed: feed.to_string(),
            at: now_secs(),
            outcome: HealthOutcome::Ok {
                records: stats.records,
                caught_up: stats.caught_up,
            },
        });
    }

    fn on_error(&mut self, feed: &str, error: &anyhow::Error) {
        // `{:#}` renders the cause chain, not just the outermost message —
        // "error decoding response body" alone would name the layer that
        // noticed rather than the venue that failed.
        self.offer(HealthUpdate {
            feed: feed.to_string(),
            at: now_secs(),
            outcome: HealthOutcome::Error(sanitize_error(&format!("{error:#}"), MAX_ERROR_CHARS)),
        });
    }
}

/// Make an error string safe to persist: strip credentials out of any URL it
/// embeds, then bound its length.
///
/// **The redaction is the load-bearing half.** A transport error's `Display`
/// routinely includes the request URL, and a keyed endpoint carries its
/// credential in the query string (`?api-key=…` is the normal shape for a
/// hosted Solana RPC, and for several price venues). That text is then written
/// to a row which — in this repo — the read-only dashboard role can `SELECT`.
/// So an error message is an exfiltration path for exactly the secrets the
/// schema's own comments promise are not in the database, and the promise is
/// unenforceable at the schema: it depends on every present and future venue
/// adapter never surfacing a keyed URL in an error. Cutting the query string
/// here enforces it in one place instead.
///
/// Only the query string goes: the scheme, host and path are what make an
/// error diagnosable, and none of them is a secret. A token carried in a path
/// segment rather than a query parameter would survive this — no adapter in
/// this repo does that, and guarding against it would mean guessing at what a
/// path segment means.
///
/// Truncation is on a **character** boundary, so multi-byte text cannot panic
/// a slice at `max`.
pub fn sanitize_error(text: &str, max: usize) -> String {
    // Whitespace-delimited, so only tokens that actually look like URLs are
    // touched; ordinary prose containing a `?` is left alone.
    let redacted = text
        .split_whitespace()
        .map(|token| match token.split_once('?') {
            Some((before, _)) if token.contains("://") => format!("{before}?<redacted>"),
            _ => token.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ");
    truncate(&redacted, max)
}

/// Truncate on a **character** boundary, so a multi-byte error message cannot
/// panic a slice at `max`.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn stats(records: usize, caught_up: bool) -> BatchStats {
        BatchStats {
            records,
            caught_up,
            fetch: Duration::ZERO,
            dispatch: Duration::ZERO,
        }
    }

    /// The consumer-side record shape: health as one variant of a wider
    /// telemetry enum, which is the arrangement `HealthReporter` is generic
    /// for.
    #[derive(Debug)]
    enum Record {
        Health(HealthUpdate),
    }

    impl From<HealthUpdate> for Record {
        fn from(update: HealthUpdate) -> Self {
            Record::Health(update)
        }
    }

    #[tokio::test]
    async fn reports_a_batch_and_an_error_through_the_consumer_record() {
        let (tx, mut rx) = mpsc::channel::<Record>(8);
        let mut reporter = HealthReporter::new(tx);

        reporter.on_batch("kraken", &stats(3, true));
        reporter.on_error("kraken", &anyhow::anyhow!("451 from the venue"));

        let Record::Health(ok) = rx.try_recv().unwrap();
        assert_eq!(ok.feed, "kraken");
        assert_eq!(
            ok.outcome,
            HealthOutcome::Ok {
                records: 3,
                caught_up: true
            }
        );

        let Record::Health(err) = rx.try_recv().unwrap();
        assert_eq!(
            err.outcome,
            HealthOutcome::Error("451 from the venue".to_string())
        );
    }

    #[tokio::test]
    async fn renders_the_whole_cause_chain() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = HealthReporter::new(tx);

        let error = anyhow::anyhow!("connection reset").context("polling EUR/USD");
        reporter.on_error("pyth-hermes", &error);

        let Record::Health(update) = rx.try_recv().unwrap();
        // The outer context alone would name the layer that noticed rather
        // than what actually failed.
        assert_eq!(
            update.outcome,
            HealthOutcome::Error("polling EUR/USD: connection reset".to_string())
        );
    }

    #[tokio::test]
    async fn a_full_channel_drops_instead_of_blocking() {
        // Capacity 1, three reports: the drive loop must not be held up, so
        // the surplus is dropped and the call still returns.
        let (tx, mut rx) = mpsc::channel::<Record>(1);
        let mut reporter = HealthReporter::new(tx);

        for _ in 0..3 {
            reporter.on_batch("coingecko", &stats(1, true));
        }
        assert_eq!(reporter.dropped, 2);

        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err(), "capacity was 1");
    }

    #[tokio::test]
    async fn a_closed_channel_is_not_an_error() {
        let (tx, rx) = mpsc::channel::<Record>(4);
        drop(rx);
        let mut reporter = HealthReporter::new(tx);

        // A consumer that has gone away must not be able to fail a feed.
        reporter.on_batch("frankfurter", &stats(1, true));
        assert_eq!(reporter.dropped, 1);
    }

    /// The redaction is the half that matters: a keyed endpoint's credential
    /// rides in the query string, and this text is persisted to a column a
    /// read-only dashboard role can read.
    #[test]
    fn sanitize_strips_url_query_strings_but_keeps_the_diagnosis() {
        let got = sanitize_error(
            "polling failed: https://rpc.example/v1?api-key=SECRET returned 500",
            MAX_ERROR_CHARS,
        );
        assert!(!got.contains("SECRET"), "got: {got}");
        assert!(got.contains("https://rpc.example/v1?<redacted>"));
        // Scheme, host and path survive — they are what makes it diagnosable
        // and none of them is a secret.
        assert!(got.contains("returned 500"));
    }

    #[test]
    fn sanitize_leaves_ordinary_prose_containing_a_question_mark_alone() {
        // Only tokens that look like URLs are touched, so an error message
        // that merely contains a `?` is not mangled.
        let text = "who knows why? the venue returned nothing";
        assert_eq!(sanitize_error(text, MAX_ERROR_CHARS), text);
    }

    #[test]
    fn sanitize_still_truncates_after_redacting() {
        let long = format!("https://h/?k={}", "x".repeat(MAX_ERROR_CHARS * 2));
        let got = sanitize_error(&long, MAX_ERROR_CHARS);
        assert!(!got.contains('x'), "the query string went first: {got}");
        assert!(got.chars().count() <= MAX_ERROR_CHARS + 1);
    }

    #[test]
    fn truncates_long_error_text_on_a_character_boundary() {
        let text = "é".repeat(MAX_ERROR_CHARS + 10);
        let cut = truncate(&text, MAX_ERROR_CHARS);
        assert_eq!(cut.chars().count(), MAX_ERROR_CHARS + 1); // + the ellipsis
        assert!(cut.ends_with('…'));
        // Short text is returned unchanged, ellipsis included in neither case.
        assert_eq!(truncate("short", MAX_ERROR_CHARS), "short");
    }
}
