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
/// Returns `Ok(())` once **every** feed has returned `Ok`, which happens on
/// shutdown: each `run` future resolves on `SIGTERM` / `ctrl-c` independently,
/// so a stop signal drains them all. Returns the **first** error otherwise,
/// cancelling the rest — there is nothing to salvage from a partially-live
/// collector.
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

    while let Some(joined) = set.join_next().await {
        match joined {
            // A feed returned an error: the process is going down, so say
            // which one before the rest are cancelled.
            Ok((name, Err(err))) => {
                tracing::error!(feed = %name, error = %err, "feed failed; stopping collector");
                set.abort_all();
                return Err(err.context(format!("feed {name}")));
            }
            Ok((name, Ok(()))) => {
                tracing::info!(feed = %name, "feed stopped");
            }
            // A panic (or a cancellation we did not ask for) is not
            // recoverable and must not read as a clean stop.
            Err(err) => {
                set.abort_all();
                return Err(anyhow!("a feed task ended abnormally: {err}"));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    #[tokio::test]
    async fn runs_every_feed_to_completion() {
        let ran = Arc::new(AtomicUsize::new(0));
        let feeds: Vec<(String, _)> = (0..4)
            .map(|i| {
                let ran = ran.clone();
                (format!("feed-{i}"), async move {
                    ran.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })
            .collect();
        run_all(feeds).await.unwrap();
        assert_eq!(ran.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn one_failing_feed_fails_the_process_and_names_itself() {
        // The contract that matters: a collector must not keep running with a
        // dead pair, because the store's coverage is what gets watched.
        let feeds: Vec<(String, _)> = vec![
            ("healthy".to_string(), Box::pin(async {
                // Long enough that the failure below is what ends the run.
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            }) as std::pin::Pin<Box<dyn Future<Output = Result<()>> + Send>>),
            ("broken".to_string(), Box::pin(async {
                Err(anyhow!("the sink rejected a batch"))
            })),
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
