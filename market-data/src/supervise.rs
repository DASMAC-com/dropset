//! Run several feeds in one process and fail the process if any of them does.
//!
//! The framework runner ([`dropset_feeds::run`]) drives **one** source. A
//! roster collector needs several — one per product for a venue whose endpoint
//! is keyed by product, which is most of them — so something has to own the
//! set. This is that something, and it is deliberately the whole of it: no
//! restart policy, no per-feed supervision tree.
//!
//! **A failed feed takes the process down.** That is the same contract a
//! single-feed collector already had: a sink error propagates out of `run`, the
//! process exits, the orchestrator restarts it, and every feed resumes from its
//! own committed cursor (at-least-once, docs/data-feeds.md §3). Restarting one
//! feed in place would be strictly worse — a collector that keeps running with
//! two of its five pairs silently dead looks healthy to everything watching it,
//! and the store's own coverage is what the dashboard reads. Crashing is the
//! signal.
//!
//! Each feed keeps its **own cursor key**, so splitting one service per venue
//! changes nothing a resume depends on: the pairs a per-pair service used to
//! collect carry on from exactly where they were.

use anyhow::{anyhow, Result};
use std::future::Future;
use tokio::task::JoinSet;

/// Drive every feed concurrently until one fails or all finish.
///
/// Feeds are `(name, future)` pairs, the name being the feed identifier used
/// for the log line that says which one brought the process down — without it
/// a panic in one of five tasks is anonymous.
///
/// **The first feed to finish, either way, ends the process.** An `Err`
/// obviously; but an `Ok` too, and that is the part worth stating. The
/// framework runner returns `Ok` only when a shutdown signal fires, and that
/// signal reaches every feed — so the first `Ok` means "we are stopping", and
/// waiting for the remaining N-1 to notice buys nothing. Two things fall out.
/// The invariant this module claims is enforced *here* rather than resting on
/// the runner's internals: there is no path on which this function keeps
/// running with fewer feeds than it started with. And shutdown is prompt: a
/// feed blocked in its venue's request pacer (Alpha Vantage's floor is an hour)
/// cannot hold the process open past its orchestrator's stop grace period,
/// which under at-least-once delivery would only have earned a `SIGKILL`
/// anyway.
pub async fn run_all<F>(feeds: Vec<(String, F)>) -> Result<()>
where
    F: Future<Output = Result<()>> + Send + 'static,
{
    if feeds.is_empty() {
        return Err(anyhow!("no feeds to run; the roster resolved to nothing"));
    }
    let count = feeds.len();
    let mut set = JoinSet::new();
    for (name, fut) in feeds {
        set.spawn(async move { (name, fut.await) });
    }
    tracing::info!(feeds = count, "collector running");

    // The first completion of any kind is terminal — see the doc above for why
    // that is the correct reading of an `Ok` and not merely a shortcut.
    match set.join_next().await {
        // A feed returned an error: the process is going down, so say which one
        // before the rest are cancelled.
        Some(Ok((name, Err(err)))) => {
            tracing::error!(feed = %name, error = %err, "feed failed; stopping collector");
            set.abort_all();
            Err(err.context(format!("feed {name}")))
        }
        Some(Ok((name, Ok(())))) => {
            tracing::info!(feed = %name, "feed stopped; shutting down the collector");
            set.abort_all();
            Ok(())
        }
        // A panic (or a cancellation we did not ask for) is not recoverable and
        // must not read as a clean stop.
        Some(Err(err)) => {
            set.abort_all();
            Err(anyhow!("a feed task ended abnormally: {err}"))
        }
        // Unreachable: the set was non-empty and this is the first join.
        None => Err(anyhow!("no feed produced a result")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    /// A heterogeneously-shaped feed future, so one test can mix a fast and a
    /// slow feed in a single `Vec`.
    type BoxedFeed = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

    #[tokio::test]
    async fn every_feed_is_spawned_before_any_completion_ends_the_run() {
        // Deliberately not `>= 1`, which `run_all` returning `Ok` already
        // implies and which therefore asserts nothing. The barrier is what makes
        // this provable under a single join: no feed can pass it until all four
        // have arrived, so reaching the assertion at all proves every feed was
        // spawned and polled — and only then does one of them return, ending
        // the run.
        let ran = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new(tokio::sync::Barrier::new(4));
        let feeds: Vec<(String, BoxedFeed)> = (0..4)
            .map(|i| {
                let ran = ran.clone();
                let gate = gate.clone();
                let fut: BoxedFeed = Box::pin(async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                    gate.wait().await;
                    // Three of the four then park; the fourth returns and takes
                    // the run down with it.
                    if i != 0 {
                        tokio::time::sleep(Duration::from_secs(3600)).await;
                    }
                    Ok(())
                });
                (format!("feed-{i}"), fut)
            })
            .collect();
        run_all(feeds).await.unwrap();
        assert_eq!(
            ran.load(Ordering::SeqCst),
            4,
            "every feed must be spawned, not just the one that finishes"
        );
    }

    #[tokio::test]
    async fn the_first_clean_stop_ends_the_run_without_waiting_for_the_rest() {
        // The runner returns Ok only on a shutdown signal, and that signal
        // reaches every feed — so the first Ok means "we are stopping", and
        // waiting for a feed parked in its venue's request pacer (an hour, on
        // one venue) would just invite a SIGKILL. The slow feed here stands in
        // for that parked one: the run must not wait on it.
        let finished_slow = Arc::new(AtomicUsize::new(0));
        let slow_marker = finished_slow.clone();
        let feeds: Vec<(String, BoxedFeed)> = vec![
            (
                "parked".to_string(),
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    slow_marker.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            ),
            ("stopped".to_string(), Box::pin(async { Ok(()) })),
        ];
        run_all(feeds).await.unwrap();
        assert_eq!(
            finished_slow.load(Ordering::SeqCst),
            0,
            "the parked feed must be cancelled, not awaited"
        );
    }

    #[tokio::test]
    async fn one_failing_feed_fails_the_process_and_names_itself() {
        // The contract that matters: a collector must not keep running with a
        // dead pair, because the store's coverage is what gets watched.
        let feeds: Vec<(String, _)> = vec![
            (
                "healthy".to_string(),
                Box::pin(async {
                    // Long enough that the failure below is what ends the run.
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Ok(())
                }) as BoxedFeed,
            ),
            (
                "broken".to_string(),
                Box::pin(async { Err(anyhow!("the sink rejected a batch")) }),
            ),
        ];
        let err = run_all(feeds).await.unwrap_err();
        let rendered = format!("{err:#}");
        assert!(rendered.contains("broken"), "{rendered}");
        assert!(rendered.contains("the sink rejected a batch"), "{rendered}");
    }

    #[tokio::test]
    async fn an_empty_roster_is_a_startup_failure() {
        // A collector with nothing to poll would sit there looking healthy.
        let feeds: Vec<(String, std::future::Ready<Result<()>>)> = vec![];
        assert!(run_all(feeds).await.is_err());
    }

    #[tokio::test]
    async fn a_panicking_feed_does_not_read_as_a_clean_stop() {
        let feeds: Vec<(String, _)> = vec![("boom".to_string(), async {
            panic!("feed panicked");
        })];
        let err = run_all(feeds).await.unwrap_err().to_string();
        assert!(err.contains("abnormally"), "{err}");
    }
}
