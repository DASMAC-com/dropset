//! Per-transport liveness for a **push** source, reported by the producer
//! itself rather than by the runner.
//!
//! [`crate::HealthReporter`] covers every *polled* source generically, and
//! deliberately does not cover a push one. The reason is not an omission to
//! close: [`crate::ChannelSource::next`] blocks until a record arrives, so the
//! runner reports a batch only when the transport *delivers* one. For a fill
//! subscription that makes the health row's last-success timestamp track the
//! last **trade**, not the last time the socket was known good — and on a
//! market with no fills for half an hour the row ages out and the generic
//! stale-feed rule pages about a price feed that is fine. No threshold repairs
//! that, because silence is a push source's healthy state, so nothing in the
//! record stream distinguishes a quiet market from a dead socket.
//!
//! What *does* distinguish them is the transport, and only the producer can
//! see it: it is the code that opened the socket, watched it close, and slept
//! before re-opening it. So this seam is driven from the producer's own thread
//! (`up` / `down` / `failed`) rather than from the drive loop, and what it
//! reports is a **connection state**, never a message recency. An operator
//! alerts on "not up", and a quiet market is not an alert at any duration.
//!
//! ## Why this cannot be dropped as casually as a health update
//!
//! A health update is *level-triggered*: the next poll restates the same
//! liveness a moment later, so [`crate::damped`] dropping one under a full
//! channel costs resolution and nothing else. A transition here is
//! *edge-triggered* — a socket goes down once — so a dropped `down` leaves the
//! row reading `up` for as long as the process lives, which is precisely the
//! silent-green failure this module exists to remove. Two mechanisms answer
//! that, and neither is a threshold:
//!
//! - an undelivered transition is **retained and retried by
//!   [`LivenessReporter::reassert`]**, which the producer's reconnect loop
//!   calls once per cycle — bounding a dropped update's staleness by one
//!   reconnect delay rather than by the process lifetime. Note the retry is
//!   `reassert`'s job *alone*: a subsequent transition **replaces** the
//!   retained one rather than carrying it, since the row is last-state-wins.
//!   The final state is therefore correct either way, but a consumer that
//!   counts transitions loses one, so a producer must `reassert` before its
//!   next transition — which is the order the reconnect loop uses;
//! - the reporter's [`Drop`] flushes any retained transition and then reports
//!   `down` if the row would otherwise be left reading `up`, so a producer
//!   thread that *ends* — including by panicking, which unwinds — is as visible
//!   as a socket that closed. That path is not hypothetical: it is the one case
//!   a dropped-sender idle source cannot distinguish from a quiet one.

use crate::damped::DampedSender;
use crate::health::{sanitize_error, MAX_ERROR_CHARS};
use crate::time::now_secs;
use tokio::sync::mpsc;

/// What a [`Drop`]-reported outage says, when the producer ended without
/// reporting one itself.
const PRODUCER_ENDED: &str = "the transport producer ended without closing the link";

/// A push transport's connection state, as its own producer observes it.
///
/// Two states, not three: an operator's question is "is this link carrying
/// traffic", and a failed *subscribe* and a dropped *socket* are the same
/// answer to it. Why it is down is a diagnosis, so it rides as `reason` rather
/// than as a third state nothing would branch on differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkState {
    /// The transport is established — subscribed, and able to deliver.
    ///
    /// Emphatically **not** "has delivered recently". A source reporting `Up`
    /// through a completely silent hour is reporting correctly.
    Up,
    /// The transport is not delivering.
    ///
    /// `reason` carries why when the producer knows — a failed subscribe, or
    /// the producer ending — and is `None` for a socket that simply closed,
    /// which is a state and not an error. The two are kept apart because a
    /// consumer must not overwrite a retained diagnosis with a null on the
    /// next clean close.
    ///
    /// **Whatever constructs this must sanitize the text first.** A consumer
    /// persists it, and the one in this workspace writes it to a column a
    /// read-only dashboard role can `SELECT`. Every route through
    /// [`LivenessReporter`] does so — [`LivenessReporter::failed`] applies
    /// [`sanitize_error`] and the [`Drop`] guard uses a fixed constant — but
    /// the variant is `pub`, so a caller building one directly carries the
    /// obligation itself.
    Down { reason: Option<String> },
}

/// One observation of one push transport's connection state.
#[derive(Clone, Debug)]
pub struct LivenessUpdate {
    /// The source's [`crate::Source::name`] — the key a consumer upserts on,
    /// matching how [`crate::HealthUpdate`] keys its own row so the two are
    /// joinable.
    pub feed: String,
    /// When the transition was observed, as an epoch second.
    ///
    /// Stamped when the state was **set**, not when it was delivered, so a
    /// retried transition still reports when the socket actually dropped.
    pub at: i64,
    /// The state the transport moved into.
    pub state: LinkState,
}

/// Reports one push transport's connection state onto a channel, for a
/// consumer that persists or renders it.
///
/// Generic over the consumer's record type on the same reasoning as
/// [`crate::HealthReporter`]: a consumer funnelling several kinds of telemetry
/// down one channel — and so writing them in one transaction, through one
/// [`crate::StoreWriter`] — carries liveness as one variant of its own enum.
///
/// Built **per source**, so it holds its feed name rather than taking one per
/// call: unlike the runner, which drives many sources through one recorder, a
/// producer thread owns exactly one transport.
///
/// Every method is infallible, deliberately: a liveness report able to fail
/// its caller would be able to take down the socket it reports on.
///
/// The `From<LivenessUpdate>` bound sits on the **struct**, where
/// [`crate::HealthReporter`] carries its equivalent on the `impl` instead. That
/// is not a stylistic divergence: this type has a [`Drop`] impl, and Rust
/// requires a `Drop` impl to repeat the struct's own bounds exactly, so the
/// bound has to be declared here for the guard to be able to build a record at
/// all.
pub struct LivenessReporter<R: From<LivenessUpdate>> {
    sender: DampedSender<R>,
    feed: String,
    /// The most recent state set — what the consumer's row *should* show.
    /// `None` before the first transition.
    latest: Option<LivenessUpdate>,
    /// Whether `latest` reached the channel. `false` with a `Some(latest)` is
    /// the retry-pending state; see the module docs for why a drop here is not
    /// self-healing the way a health update's is.
    delivered: bool,
}

impl<R: From<LivenessUpdate>> LivenessReporter<R> {
    /// Report `feed`'s transport state onto `tx`. Bound the channel when
    /// building it: this drops rather than waits, so a stalled telemetry drain
    /// cannot hold up a reconnect.
    pub fn new(tx: mpsc::Sender<R>, feed: impl Into<String>) -> Self {
        Self {
            sender: DampedSender::new(tx, "liveness"),
            feed: feed.into(),
            latest: None,
            delivered: true,
        }
    }

    /// The transport is established and able to deliver.
    ///
    /// Call this on a successful subscribe, not on the first record: the point
    /// of the whole seam is that "able to deliver" and "did deliver" are
    /// different facts, and only the former is a health signal.
    pub fn up(&mut self) {
        self.set(LinkState::Up);
    }

    /// The transport closed. No diagnosis: a socket the venue closed is a
    /// state, not an error, and a consumer must leave any retained `reason`
    /// from an earlier failure alone rather than null it out.
    pub fn down(&mut self) {
        self.set(LinkState::Down { reason: None });
    }

    /// The transport could not be established, or failed while up.
    ///
    /// The error text is put through [`sanitize_error`], which strips URL
    /// query strings wholesale — and unlike the health path, that **is** the
    /// right instrument here. A push producer's error comes from its own
    /// client (the Solana `PubsubClient`, a raw WebSocket), none of which
    /// passes through [`crate::HttpClient`] and so none of which reaches its
    /// [`redact_query`](crate::HttpClient) pass at all, and a hosted endpoint
    /// carries its credential as `?api-key=…` exactly as a price venue does.
    /// That text lands in a column the read-only dashboard
    /// role can `SELECT`, so without the blunt strip an error message is an
    /// exfiltration path for the credential. Losing the benign query
    /// parameters with it costs nothing here: a subscribe URL's query string
    /// carries no diagnosis worth the risk, which is the opposite of the paged
    /// backfill the health path is careful about.
    pub fn failed(&mut self, error: &anyhow::Error) {
        // `{:#}` renders the cause chain, not just the outermost message —
        // otherwise the layer that noticed is named rather than what failed.
        let reason = sanitize_error(&format!("{error:#}"), MAX_ERROR_CHARS);
        self.set(LinkState::Down {
            reason: Some(reason),
        });
    }

    /// Retry a transition the channel refused, if there is one.
    ///
    /// A no-op when the latest state was delivered, so a producer can call it
    /// unconditionally once per loop — which is the intended use, and what
    /// bounds a dropped transition's lifetime to one reconnect cycle instead
    /// of the process's.
    pub fn reassert(&mut self) {
        self.flush();
    }

    fn set(&mut self, state: LinkState) {
        self.latest = Some(LivenessUpdate {
            feed: self.feed.clone(),
            at: now_secs(),
            state,
        });
        self.delivered = false;
        self.flush();
    }

    fn flush(&mut self) {
        if self.delivered {
            return;
        }
        // Cloned rather than taken: a refused offer must leave `latest` intact
        // for the next attempt, and `latest` is also what `Drop` reads to
        // decide whether the row would be left reading `Up`.
        let Some(update) = self.latest.clone() else {
            return;
        };
        self.delivered = self.sender.offer(&self.feed, R::from(update));
    }
}

/// Retry any undelivered transition, then report `down` if the consumer's row
/// would otherwise be left reading `up`.
///
/// The producer ending is the failure mode a push source cannot otherwise
/// signal: when its thread returns or panics (which unwinds, so this runs),
/// the record channel's sender drops and [`crate::ChannelSource`] starts
/// yielding *empty, caught-up* batches — indistinguishable, by design, from a
/// transport that is simply quiet. Without this the last thing said about a
/// dead subscription is that it was healthy.
///
/// **The flush is not an optimization — it closes the gap this guard would
/// otherwise open.** Every *other* refused transition is retried by
/// [`LivenessReporter::reassert`] on the producer's next loop; a producer that
/// is ending has no next loop, so this is the last moment its retained update
/// can ever be sent. Deciding without flushing keys the guard on the last state
/// *set* rather than the last state *delivered*, and then the one sequence that
/// matters — `up` lands, the socket dies, the `down` is refused by a full
/// channel, the thread ends — discards that `down` and leaves the row reading
/// `up` for the life of the process. That is precisely the silent-green failure
/// this module exists to remove, arrived at through the code meant to prevent
/// it.
///
/// Once the flush has run, `delivered` says whether `latest` is what the
/// consumer actually holds, so the guard below can ask the question it means to
/// ask. A still-undelivered state means the channel is full or gone, in which
/// case synthesizing another update would fail too — so it correctly declines.
///
/// Best-effort by nature: there is no later call to retry from, so a channel
/// that stays full through this instant loses the line. That is strictly better
/// than not reporting at all, and the window is one process's final moment.
impl<R: From<LivenessUpdate>> Drop for LivenessReporter<R> {
    fn drop(&mut self) {
        self.flush();
        let delivered_up = matches!(
            (self.delivered, self.latest.as_ref().map(|u| &u.state)),
            (true, Some(LinkState::Up))
        );
        if !delivered_up {
            return;
        }
        self.set(LinkState::Down {
            reason: Some(PRODUCER_ENDED.to_string()),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The consumer-side record shape: liveness as one variant of a wider
    /// telemetry enum, which is the arrangement the reporter is generic for.
    #[derive(Debug)]
    enum Record {
        Liveness(LivenessUpdate),
    }

    impl From<LivenessUpdate> for Record {
        fn from(update: LivenessUpdate) -> Self {
            Record::Liveness(update)
        }
    }

    fn state(rx: &mut mpsc::Receiver<Record>) -> LinkState {
        let Record::Liveness(update) = rx.try_recv().expect("a liveness update");
        update.state
    }

    #[tokio::test]
    async fn reports_each_transition_under_the_source_name() {
        let (tx, mut rx) = mpsc::channel::<Record>(8);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        let Record::Liveness(update) = rx.try_recv().unwrap();
        assert_eq!(update.feed, "maker-fills");
        assert_eq!(update.state, LinkState::Up);

        reporter.down();
        assert_eq!(state(&mut rx), LinkState::Down { reason: None });
    }

    /// A clean close carries no reason, so a consumer has nothing to overwrite
    /// a retained diagnosis with. Pinned because "always send the reason, even
    /// if empty" is the tempting simplification and it nulls out the last
    /// thing an operator had to go on.
    #[tokio::test]
    async fn a_clean_close_carries_no_diagnosis() {
        let (tx, mut rx) = mpsc::channel::<Record>(8);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.failed(&anyhow::anyhow!("connection refused"));
        assert_eq!(
            state(&mut rx),
            LinkState::Down {
                reason: Some("connection refused".to_string())
            }
        );

        reporter.down();
        assert_eq!(state(&mut rx), LinkState::Down { reason: None });
    }

    #[tokio::test]
    async fn a_failure_renders_the_whole_cause_chain() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.failed(&anyhow::anyhow!("connection reset").context("logs_subscribe"));

        assert_eq!(
            state(&mut rx),
            LinkState::Down {
                reason: Some("logs_subscribe: connection reset".to_string())
            }
        );
    }

    /// The security property of this seam: a producer's client has no
    /// name-aware redaction hook, so a keyed subscribe URL in the error text
    /// would otherwise reach a column the dashboard role can read.
    #[tokio::test]
    async fn a_failure_strips_a_credential_bearing_url() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.failed(&anyhow::anyhow!(
            "logs_subscribe wss://rpc.example/v1?api-key=SECRET: connection refused"
        ));

        let LinkState::Down { reason: Some(text) } = state(&mut rx) else {
            panic!("expected a diagnosed outage");
        };
        assert!(!text.contains("SECRET"), "got: {text}");
        assert!(text.contains("wss://rpc.example/v1?<redacted>"));
        // The host and path survive, which is what makes it diagnosable.
        assert!(text.contains("connection refused"));
    }

    /// The edge-triggered property: a refused transition must not be lost, or
    /// the row reads `up` for the life of the process.
    #[tokio::test]
    async fn a_refused_transition_is_retried_not_lost() {
        let (tx, mut rx) = mpsc::channel::<Record>(1);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        // Fill the channel, then transition: the `down` is refused.
        reporter.up();
        reporter.down();
        assert!(!reporter.delivered, "capacity was 1");

        // Drain the `up`, then let the producer's next loop reassert.
        assert_eq!(state(&mut rx), LinkState::Up);
        reporter.reassert();
        assert!(reporter.delivered);
        assert_eq!(state(&mut rx), LinkState::Down { reason: None });
    }

    /// And the retry carries the transition's **own** timestamp, not the
    /// retry's — otherwise the row would claim the socket dropped later than
    /// it did.
    #[tokio::test]
    async fn a_retried_transition_keeps_its_original_timestamp() {
        let (tx, mut rx) = mpsc::channel::<Record>(1);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        reporter.down();
        let dropped_at = reporter.latest.as_ref().unwrap().at;

        assert_eq!(state(&mut rx), LinkState::Up);
        reporter.reassert();

        let Record::Liveness(update) = rx.try_recv().unwrap();
        assert_eq!(update.at, dropped_at);
    }

    /// `reassert` is called once per producer loop, so it has to be free when
    /// there is nothing outstanding.
    #[tokio::test]
    async fn reassert_is_a_noop_once_delivered() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        assert_eq!(state(&mut rx), LinkState::Up);

        reporter.reassert();
        reporter.reassert();
        assert!(
            rx.try_recv().is_err(),
            "a delivered state must not be re-sent"
        );
    }

    /// A producer that ends while the row reads `up` must not leave it there —
    /// this is the thread-death path a dropped record sender cannot express.
    #[tokio::test]
    async fn dropping_the_reporter_closes_a_link_left_reading_up() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        assert_eq!(state(&mut rx), LinkState::Up);
        drop(reporter);

        let LinkState::Down { reason: Some(text) } = state(&mut rx) else {
            panic!("expected a diagnosed outage");
        };
        assert_eq!(text, PRODUCER_ENDED);
    }

    /// But a producer that reported its own outage first says nothing extra —
    /// otherwise every clean shutdown would overwrite the real diagnosis.
    #[tokio::test]
    async fn dropping_after_a_reported_outage_adds_nothing() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        reporter.failed(&anyhow::anyhow!("connection refused"));
        assert_eq!(state(&mut rx), LinkState::Up);
        assert!(matches!(
            state(&mut rx),
            LinkState::Down { reason: Some(_) }
        ));

        drop(reporter);
        assert!(rx.try_recv().is_err(), "the outage was already reported");
    }

    /// A reporter that never came up says nothing on drop either: there is no
    /// row claiming health to correct.
    #[tokio::test]
    async fn dropping_a_reporter_that_never_came_up_says_nothing() {
        let (tx, mut rx) = mpsc::channel::<Record>(4);
        let reporter = LivenessReporter::<Record>::new(tx, "maker-fills");

        drop(reporter);
        assert!(rx.try_recv().is_err());
    }

    /// A dead drain must not be able to fail or stall the producer, which is
    /// the same contract the health seam holds.
    #[tokio::test]
    async fn a_dead_drain_cannot_fail_the_producer() {
        let (tx, rx) = mpsc::channel::<Record>(4);
        drop(rx);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        reporter.down();
        reporter.failed(&anyhow::anyhow!("gone"));
        reporter.reassert();
        assert!(!reporter.delivered);
    }

    /// The sequence the `Drop` guard exists for, and the one it used to get
    /// wrong: the link came up, went down, and the `down` was **refused** by a
    /// full channel — then the producer ended.
    ///
    /// A guard that keys on the last state *set* sees `Down`, concludes there
    /// is nothing to correct, and discards the retained transition at the last
    /// moment it could ever be sent, leaving the consumer's row reading `up`
    /// forever. Flushing first is what makes the outage visible. There is no
    /// `reassert` to fall back on here — a producer that is ending has no next
    /// loop, which is exactly why this case is the guard's job.
    #[tokio::test]
    async fn dropping_with_an_undelivered_outage_still_reports_it() {
        let (tx, mut rx) = mpsc::channel::<Record>(1);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        reporter.down();
        assert!(!reporter.delivered, "the down was refused, capacity is 1");

        // The drain catches up, as it would while a dying thread unwinds.
        assert_eq!(state(&mut rx), LinkState::Up);
        drop(reporter);

        assert_eq!(
            state(&mut rx),
            LinkState::Down { reason: None },
            "the retained outage must reach the consumer"
        );
        // And no PRODUCER_ENDED on top: the outage was already reported, so
        // synthesizing another would overwrite a real diagnosis with a generic
        // one.
        assert!(rx.try_recv().is_err(), "exactly one report, not two");
    }

    /// The mirror of the case above: when the retained transition *cannot* be
    /// delivered either, the guard must not synthesize a second update that
    /// would fail the same way — and must not claim the link was up.
    #[tokio::test]
    async fn dropping_with_a_dead_drain_synthesizes_nothing() {
        let (tx, rx) = mpsc::channel::<Record>(4);
        let mut reporter = LivenessReporter::new(tx, "maker-fills");

        reporter.up();
        drop(rx);
        reporter.down();
        assert!(!reporter.delivered);

        // Dropping must simply return rather than looping or panicking.
        drop(reporter);
    }
}
