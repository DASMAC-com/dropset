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
//! resolution, never correctness. That property is exactly what a **push**
//! source lacks, which is why its liveness is a separate seam
//! ([`crate::LivenessReporter`]) rather than another caller of this one.
//!
//! The offer-and-damp mechanics are shared with that seam in
//! [`crate::damped`].

use crate::damped::DampedSender;
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
    sender: DampedSender<R>,
}

impl<R> HealthReporter<R> {
    /// Report onto `tx`. Bound the channel when building it: this reporter
    /// drops rather than waits, which is the property that keeps a slow drain
    /// off the drive loop.
    pub fn new(tx: mpsc::Sender<R>) -> Self {
        Self {
            sender: DampedSender::new(tx, "health"),
        }
    }
}

impl<R: From<HealthUpdate>> HealthReporter<R> {
    /// Offer one update, dropping it if the channel is full or closed.
    ///
    /// The dropped case needs no handling here, and that is the whole
    /// difference from [`crate::LivenessReporter`]: this update is
    /// level-triggered, so the next turn restates it and a drop costs
    /// resolution rather than correctness.
    fn offer(&mut self, update: HealthUpdate) {
        // Captured before `R::from` consumes the update. Every line the sender
        // logs belongs to exactly one feed — a reporter is built per source at
        // spawn time — and this crate's other logs all carry `feed`, so
        // without it an operator filtering by feed sees the price polls but
        // never the reporter that went quiet.
        let feed = update.feed.clone();
        self.sender.offer(&feed, R::from(update));
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

    /// The drop damping itself is [`crate::damped`]'s, and is tested there
    /// against `DampedSender` directly. What stays here is the one property of
    /// it this seam depends on: a drive loop must not be stalled or failed by
    /// a drain that has gone away.
    #[tokio::test]
    async fn a_dead_drain_cannot_fail_or_stall_a_feed() {
        let (tx, rx) = mpsc::channel::<Record>(1);
        drop(rx);
        let mut reporter = HealthReporter::new(tx);

        // Infallible by signature, so the assertion is that these return at
        // all — a health path able to fail would be `?`-propagated into the
        // poll it reports on.
        reporter.on_batch("frankfurter", &stats(1, true));
        reporter.on_error("frankfurter", &anyhow::anyhow!("gone"));
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
