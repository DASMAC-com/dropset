//! The runner: drive a source and fan each batch to its sinks.

// cspell:word oneshot

use crate::sink::Sink;
use crate::source::Source;
use anyhow::Result;
use std::future::Future;
use std::time::Duration;
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

/// Drive `source`, fanning each batch to every sink, until `ctrl-c` /
/// `SIGTERM`. The shutdown-injectable core is [`run_until`].
pub async fn run<S: Source>(
    source: S,
    sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
) -> Result<()> {
    run_until(source, sinks, cfg, shutdown_signal()).await
}

/// The runner core with an injectable `shutdown` future (the unit-testable
/// seam). Loops tight while the source is backfilling, sleeps `poll_interval`
/// when caught up, backs off `error_backoff` on a source error, and returns
/// when `shutdown` resolves. A sink error propagates out — the process is
/// meant to crash and resume from the store cursor.
pub async fn run_until<S, F>(
    mut source: S,
    mut sinks: Vec<Box<dyn Sink<S::Record>>>,
    cfg: RunConfig,
    shutdown: F,
) -> Result<()>
where
    S: Source,
    F: Future<Output = ()>,
{
    // Name is stable; clone once so logging never borrows `source` while a
    // `source.next()` future in the `select!` still holds it mutably.
    let name = source.name().to_string();
    tokio::pin!(shutdown);
    loop {
        let batch = tokio::select! {
            biased;
            _ = &mut shutdown => break,
            result = source.next() => match result {
                Ok(batch) => batch,
                Err(err) => {
                    tracing::warn!(feed = %name, error = %err, "source failed; backing off");
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = sleep(cfg.error_backoff) => continue,
                    }
                }
            },
        };
        for sink in sinks.iter_mut() {
            sink.handle(&batch).await?;
        }
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
                Batch::new(vec![1, 2]).caught_up(false),
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
}
