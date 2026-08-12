//! The runner: drive a source and fan each batch to its sinks.

// cspell:word oneshot

use crate::sink::Sink;
use crate::source::Source;
use anyhow::Result;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::time::sleep;

/// Runner timing.
#[derive(Clone, Debug)]
pub struct RunConfig {
    /// Sleep between polls once the source reports it is caught up.
    pub poll_interval: Duration,
    /// Sleep after a source error before retrying.
    pub error_backoff: Duration,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            error_backoff: Duration::from_secs(5),
        }
    }
}

/// What the runner observed for one turn of the drive loop, handed to a
/// [`FeedMetrics`] recorder.
///
/// **On cursor lag.** The framework cannot measure how far behind a feed is
/// in the source's own units — only the source knows whether its position is
/// a timestamp, a slot, or a signature. What it *can* report is
/// [`BatchStats::caught_up`]: `false` means the source is still draining a
/// backlog. A recorder derives lag from that (consecutive behind-turns, or
/// records drained per turn) plus whatever the source exposes itself.
#[derive(Clone, Debug)]
pub struct BatchStats {
    /// Records in this batch.
    pub records: usize,
    /// Whether the source reported it has reached the present.
    pub caught_up: bool,
    /// Wall time spent in [`Source::next`].
    pub fetch: Duration,
    /// Wall time spent fanning the batch to every sink.
    pub dispatch: Duration,
}

/// The observability seam the runner emits through, so a deployed feed is
/// instrumented without per-feed wiring (docs/data-feeds.md §7).
///
/// Implementations are called inline on the drive loop and must not block:
/// increment a counter, push to a channel, and return. The default
/// implementations do nothing, so a recorder only overrides what it cares
/// about, and [`NoopMetrics`] costs nothing for the consumers that want none.
pub trait FeedMetrics: Send {
    /// One batch was fetched and fanned out successfully.
    fn on_batch(&mut self, feed: &str, stats: &BatchStats) {
        let _ = (feed, stats);
    }

    /// [`Source::next`] failed; the runner is about to back off and retry.
    /// The error rate is this callback's frequency against `on_batch`'s.
    fn on_error(&mut self, feed: &str, error: &anyhow::Error) {
        let _ = (feed, error);
    }
}

/// The default recorder: records nothing. What [`run`] and [`run_until`] use.
pub struct NoopMetrics;

impl FeedMetrics for NoopMetrics {}

/// Drive `source`, fanning each batch to every sink, until `ctrl-c` /
/// `SIGTERM`. The shutdown-injectable core is [`run_until`].
pub async fn run<S: Source>(
    source: S,
    sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
) -> Result<()> {
    run_until(source, sinks, cfg, shutdown_signal()).await
}

/// [`run`], reporting each batch and source error to `metrics`.
pub async fn run_with_metrics<S: Source, M: FeedMetrics>(
    source: S,
    sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
    metrics: M,
) -> Result<()> {
    run_until_with_metrics(source, sinks, cfg, shutdown_signal(), metrics).await
}

/// The runner core with an injectable `shutdown` future (the unit-testable
/// seam). Loops tight while the source is backfilling, sleeps `poll_interval`
/// when caught up, backs off `error_backoff` on a source error, and returns
/// when `shutdown` resolves. A sink error propagates out — the process is
/// meant to crash and resume from the store cursor.
pub async fn run_until<S, F>(
    source: S,
    sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
    shutdown: F,
) -> Result<()>
where
    S: Source,
    F: Future<Output = ()>,
{
    run_until_with_metrics(source, sinks, cfg, shutdown, NoopMetrics).await
}

/// [`run_until`], reporting each batch and source error to `metrics`.
pub async fn run_until_with_metrics<S, F, M>(
    mut source: S,
    mut sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
    shutdown: F,
    mut metrics: M,
) -> Result<()>
where
    S: Source,
    F: Future<Output = ()>,
    M: FeedMetrics,
{
    // Name is stable; clone once so logging never borrows `source` while a
    // `source.next()` future in the `select!` still holds it mutably.
    let name = source.name().to_string();
    tokio::pin!(shutdown);
    loop {
        let started = Instant::now();
        let batch = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            result = source.next() => match result {
                Ok(batch) => batch,
                Err(err) => {
                    tracing::warn!(feed = %name, error = %err, "source failed; backing off");
                    metrics.on_error(&name, &err);
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = sleep(cfg.error_backoff) => continue,
                    }
                }
            },
        };
        let fetch = started.elapsed();
        let dispatch_started = Instant::now();
        for sink in sinks.iter_mut() {
            sink.handle(&batch).await?;
        }
        metrics.on_batch(
            &name,
            &BatchStats {
                records: batch.len(),
                caught_up: batch.caught_up,
                fetch,
                dispatch: dispatch_started.elapsed(),
            },
        );
        if batch.caught_up {
            tokio::select! {
                _ = &mut shutdown => break,
                _ = sleep(cfg.poll_interval) => {}
            }
        }
    }
    tracing::info!(feed = %name, "feed shutting down");
    Ok(())
}

/// Resolves on `ctrl-c` or, on Unix, `SIGTERM` (the ECS / compose stop
/// signal).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::forward_channel;
    use crate::record::Batch;
    use crate::sink::Sink;
    use crate::source::Source;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use tokio::sync::oneshot;

    /// A source that replays a script of batches, then signals `done` and idles
    /// with empty caught-up batches.
    struct MockSource {
        name: String,
        batches: VecDeque<Batch<u64>>,
        done: Option<oneshot::Sender<()>>,
    }

    #[async_trait]
    impl Source for MockSource {
        type Record = u64;
        fn name(&self) -> &str {
            &self.name
        }
        async fn next(&mut self) -> Result<Batch<u64>> {
            if let Some(batch) = self.batches.pop_front() {
                Ok(batch)
            } else {
                if let Some(done) = self.done.take() {
                    let _ = done.send(());
                }
                Ok(Batch::new(vec![]))
            }
        }
    }

    #[tokio::test]
    async fn fans_out_scripted_batches_then_shuts_down() {
        let (fwd, mut rx) = forward_channel::<u64>(64);
        let (done_tx, done_rx) = oneshot::channel();
        let source = MockSource {
            name: "mock".into(),
            // A backlog batch (caught_up = false → loop immediately) then a
            // caught-up batch.
            batches: VecDeque::from(vec![
                Batch::new(vec![1, 2]).with_caught_up(false),
                Batch::new(vec![3]),
            ]),
            done: Some(done_tx),
        };
        let sinks: Vec<Box<dyn Sink<u64>>> = vec![Box::new(fwd)];
        let cfg = RunConfig {
            poll_interval: Duration::from_millis(5),
            error_backoff: Duration::from_millis(5),
        };

        run_until(source, sinks, cfg, async move {
            let _ = done_rx.await;
        })
        .await
        .unwrap();

        let mut got = Vec::new();
        while let Ok(v) = rx.try_recv() {
            got.push(v);
        }
        assert_eq!(got, vec![1, 2, 3]);
    }

    /// A source whose first `next()` errors, then succeeds — the runner should
    /// back off and retry rather than give up.
    struct FlakySource {
        name: String,
        failed: bool,
        done: Option<oneshot::Sender<()>>,
    }

    #[async_trait]
    impl Source for FlakySource {
        type Record = u64;
        fn name(&self) -> &str {
            &self.name
        }
        async fn next(&mut self) -> Result<Batch<u64>> {
            if !self.failed {
                self.failed = true;
                anyhow::bail!("transient");
            }
            if let Some(done) = self.done.take() {
                let _ = done.send(());
                return Ok(Batch::new(vec![42]));
            }
            Ok(Batch::new(vec![]))
        }
    }

    #[tokio::test]
    async fn backs_off_and_retries_after_a_source_error() {
        let (fwd, mut rx) = forward_channel::<u64>(16);
        let (done_tx, done_rx) = oneshot::channel();
        let source = FlakySource {
            name: "flaky".into(),
            failed: false,
            done: Some(done_tx),
        };
        let sinks: Vec<Box<dyn Sink<u64>>> = vec![Box::new(fwd)];
        let cfg = RunConfig {
            poll_interval: Duration::from_millis(5),
            error_backoff: Duration::from_millis(5),
        };

        run_until(source, sinks, cfg, async move {
            let _ = done_rx.await;
        })
        .await
        .unwrap();

        assert_eq!(rx.try_recv().unwrap(), 42);
    }

    /// A recorder that keeps what the runner reported, shared so the test can
    /// read it back after the runner has consumed the recorder.
    #[derive(Clone, Default)]
    struct Recorder {
        batches: Arc<Mutex<Vec<(String, usize, bool)>>>,
        errors: Arc<Mutex<Vec<String>>>,
    }

    impl FeedMetrics for Recorder {
        fn on_batch(&mut self, feed: &str, stats: &BatchStats) {
            self.batches
                .lock()
                .unwrap()
                .push((feed.to_string(), stats.records, stats.caught_up));
        }
        fn on_error(&mut self, feed: &str, error: &anyhow::Error) {
            self.errors.lock().unwrap().push(format!("{feed}: {error}"));
        }
    }

    #[tokio::test]
    async fn reports_each_batch_and_source_error_to_the_metrics_seam() {
        let (fwd, _rx) = forward_channel::<u64>(64);
        let (done_tx, done_rx) = oneshot::channel();
        // Errors first, then a backlog batch, then a caught-up one — so the
        // recorder sees one error and two distinct `caught_up` states.
        let source = ScriptedSource {
            name: "metered".into(),
            steps: VecDeque::from(vec![
                Step::Fail,
                Step::Yield(Batch::new(vec![1, 2]).with_caught_up(false)),
                Step::Yield(Batch::new(vec![3])),
            ]),
            done: Some(done_tx),
        };
        let recorder = Recorder::default();
        let sinks: Vec<Box<dyn Sink<u64>>> = vec![Box::new(fwd)];
        let cfg = RunConfig {
            poll_interval: Duration::from_millis(5),
            error_backoff: Duration::from_millis(5),
        };

        run_until_with_metrics(
            source,
            sinks,
            cfg,
            async move {
                let _ = done_rx.await;
            },
            recorder.clone(),
        )
        .await
        .unwrap();

        let errors = recorder.errors.lock().unwrap().clone();
        assert_eq!(errors, vec!["metered: scripted failure".to_string()]);

        // A failed `next()` reports only through `on_error` — no batch is
        // recorded for it — and the feed name rides along on both.
        let batches = recorder.batches.lock().unwrap().clone();
        assert_eq!(batches[0], ("metered".to_string(), 2, false));
        assert_eq!(batches[1], ("metered".to_string(), 1, true));
    }

    /// One turn of a scripted source: fail, or yield a batch.
    enum Step {
        Fail,
        Yield(Batch<u64>),
    }

    /// A source that replays a script of failures and batches, then signals
    /// `done` and idles with empty caught-up batches.
    struct ScriptedSource {
        name: String,
        steps: VecDeque<Step>,
        done: Option<oneshot::Sender<()>>,
    }

    #[async_trait]
    impl Source for ScriptedSource {
        type Record = u64;
        fn name(&self) -> &str {
            &self.name
        }
        async fn next(&mut self) -> Result<Batch<u64>> {
            match self.steps.pop_front() {
                Some(Step::Fail) => anyhow::bail!("scripted failure"),
                Some(Step::Yield(batch)) => Ok(batch),
                None => {
                    if let Some(done) = self.done.take() {
                        let _ = done.send(());
                    }
                    Ok(Batch::new(vec![]))
                }
            }
        }
    }
}
