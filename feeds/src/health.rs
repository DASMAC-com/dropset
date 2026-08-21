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
///
/// It bounds the text a truncated payload *carries*, not the string's total
/// length: the ellipsis is appended after the cut, so a truncated value is
/// `MAX_ERROR_CHARS + 1` characters. Stated because the paragraph above
/// invites sizing a column from it, and a `varchar` sized to exactly this
/// would reject every truncated error.
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

/// The shortest gap between two "updates dropped" warnings, in seconds.
///
/// The count-based damping above handles a *sustained* outage, but not a
/// flapping one: the counter resets on every success, so a drain hovering at
/// the edge of its write timeout scores `dropped == 1` on every turn and
/// earns the first-of-a-run line every time — a warning plus a recovery line
/// per tick, forever, which is the volume the damping exists to prevent. A
/// wall-clock floor bounds the flapping case without going silent on it.
const MIN_WARN_INTERVAL_SECS: i64 = 60;

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

impl<R> HealthReporter<R> {
    /// Report onto `tx`. Bound the channel when building it: this reporter
    /// drops rather than waits, which is the property that keeps a slow drain
    /// off the drive loop.
    pub fn new(tx: mpsc::Sender<R>) -> Self {
        Self {
            tx,
            dropped: 0,
            closed_reported: false,
            last_warn_at: 0,
            warned: false,
        }
    }
}

impl<R: From<HealthUpdate>> HealthReporter<R> {
    /// Offer one update, dropping it if the channel is full or closed.
    ///
    /// Deliberately infallible: a health report that could fail the caller
    /// would put the telemetry path in a position to take down the feed it is
    /// reporting on.
    fn offer(&mut self, update: HealthUpdate) {
        // Captured before `R::from` consumes the update. Every line below
        // belongs to exactly one feed — a reporter is built per source at
        // spawn time — and this crate's other logs all carry `feed`, so
        // without it an operator filtering by feed sees the price polls but
        // never the reporter that went quiet.
        let feed = update.feed.clone();
        match self.tx.try_send(R::from(update)) {
            Ok(()) => {
                if self.dropped > 0 {
                    // Only if the run was actually warned about — otherwise a
                    // flapping drain earns a recovery line for an outage the
                    // operator was never told had started.
                    if self.warned {
                        tracing::info!(
                            feed = %feed,
                            recovered_after = self.dropped,
                            "feed health updates flowing again"
                        );
                    }
                    self.dropped = 0;
                    self.warned = false;
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                // Permanent: no later turn can succeed, so say it once rather
                // than one line per N turns forever. Latched on its own flag,
                // not on `dropped` — see the field's comment for why sharing
                // the counter silences exactly the case that most needs a
                // line. Carries the count, so a `Closed` that follows a
                // `Full` run reports the whole outage rather than just its
                // tail.
                if !self.closed_reported {
                    self.closed_reported = true;
                    tracing::warn!(
                        feed = %feed,
                        dropped = self.dropped,
                        "the feed health drain is gone — health updates will \
                         be dropped for the life of this process"
                    );
                }
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
        //
        // Truncated but **not** query-stripped, deliberately. This crate's own
        // transport already redacts URL-borne credentials, by registered
        // parameter *name*, in `HttpClient::redact_query` — before the error is
        // wrapped. Running [`sanitize_error`] on top would strip the whole
        // query string and take every benign parameter with it, and those are
        // exactly what a failed paged backfill is diagnosed from: which symbol,
        // which interval, which window. The narrower name-aware redaction is
        // strictly better here, so this stays out of its way.
        self.offer(HealthUpdate {
            feed: feed.to_string(),
            at: now_secs(),
            outcome: HealthOutcome::Error(truncate(&format!("{error:#}"), MAX_ERROR_CHARS)),
        });
    }
}

/// Make an error string safe to persist: strip credentials out of any URL it
/// embeds, then bound its length.
///
/// **For a `feeds` transport error, prefer the transport's own redaction and
/// do not use this.** [`crate::HttpClient`] redacts URL-borne credentials by
/// registered parameter *name* before an error is wrapped, which keeps every
/// benign parameter legible — and those are what a failed paged backfill is
/// diagnosed from. This is the blunt instrument: it removes the whole query
/// string, diagnostics included.
///
/// It exists for the error text a consumer persists from a client that has
/// **no** such hook. The maker's tick error is the case: it comes from the
/// Solana RPC client, and a hosted keyed endpoint carries its credential as
/// `?api-key=…` there exactly as a price venue does. That text lands in a
/// column the read-only dashboard role can `SELECT`, so without something
/// here an error message is an exfiltration path for precisely the secret the
/// schema's comments promise is not in the database — and at the schema that
/// promise is unenforceable, since it would depend on every present and future
/// caller never surfacing a keyed URL.
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
        assert!(reporter.closed_reported, "the permanent case must be said");
    }

    /// A drain that falls behind and *then* dies is the likely order — the
    /// drain slow enough to fill the channel is the one most likely to fail
    /// next — and it is the case a shared latch silences: the `Closed` arm
    /// would see a non-zero drop count and say nothing, leaving "the drain is
    /// behind" (transient) as the operator's last word on permanent loss.
    #[tokio::test]
    async fn a_full_run_does_not_swallow_the_permanent_closure_line() {
        let (tx, rx) = mpsc::channel::<Record>(1);
        let mut reporter = HealthReporter::new(tx);

        // Fall behind first: capacity 1, so the surplus drops and the counter
        // leaves zero behind.
        for _ in 0..3 {
            reporter.on_batch("kraken", &stats(1, true));
        }
        assert_eq!(reporter.dropped, 2);
        assert!(!reporter.closed_reported, "still merely behind");

        // Then the drain dies.
        drop(rx);
        reporter.on_batch("kraken", &stats(1, true));

        assert!(
            reporter.closed_reported,
            "a preceding Full run must not suppress the permanent line"
        );
        // And the count carries the whole outage, not just its tail.
        assert_eq!(reporter.dropped, 3);
    }

    /// The count-based damping resets on every success, so a flapping drain
    /// scores `dropped == 1` every turn and would earn the first-of-a-run
    /// warning every turn. The wall-clock floor is what bounds it.
    #[tokio::test]
    async fn a_flapping_drain_is_warned_about_once_not_every_turn() {
        let (tx, mut rx) = mpsc::channel::<Record>(1);
        let mut reporter = HealthReporter::new(tx);

        // First drop of the first run: warned, and the clock is stamped.
        reporter.on_batch("oanda", &stats(1, true));
        reporter.on_batch("oanda", &stats(1, true));
        assert!(reporter.warned);
        let stamped = reporter.last_warn_at;

        // Drain, then flap again inside the interval.
        assert!(rx.try_recv().is_ok());
        reporter.on_batch("oanda", &stats(1, true));
        assert_eq!(reporter.dropped, 0, "the success reset the run");
        assert!(!reporter.warned, "and cleared the outstanding warning");

        reporter.on_batch("oanda", &stats(1, true));
        assert_eq!(reporter.dropped, 1, "a fresh first-of-run drop");
        assert!(
            !reporter.warned,
            "but inside MIN_WARN_INTERVAL_SECS it stays quiet"
        );
        assert_eq!(reporter.last_warn_at, stamped, "and does not re-stamp");
    }

    /// The health path must NOT blanket-strip query strings, because the
    /// transport has already redacted the credential by name and the remaining
    /// parameters are the diagnosis. Pinned as its own test because the
    /// tempting "belt-and-braces" change here silently destroys them.
    #[tokio::test]
    async fn a_reported_error_keeps_its_benign_query_parameters() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = HealthReporter::new(tx);

        // What the transport hands over: the credential already replaced by
        // name, the window parameters intact.
        reporter.on_error(
            "oanda",
            &anyhow::anyhow!("GET https://api.test/candles?granularity=60&apikey=REDACTED failed"),
        );

        let Record::Health(update) = rx.try_recv().unwrap();
        let HealthOutcome::Error(text) = update.outcome else {
            panic!("expected an error outcome");
        };
        assert!(
            text.contains("granularity=60"),
            "the diagnosis must survive: {text}"
        );
        assert!(text.contains("REDACTED"), "and the redaction is upstream's");
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
