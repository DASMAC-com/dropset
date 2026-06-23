//! Background-job harness. Actions that touch the chain or shell out
//! (deploy, bootstrap steps, teardown, wipe) run on a `std::thread` and
//! stream [`JobEvent`]s back over an `mpsc` channel, so the synchronous
//! event loop never blocks on an RPC round-trip or an `anchor build`.

use anyhow::{bail, Context, Result};
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

/// A message from a running background job to the event loop.
pub enum JobEvent {
    /// A line for the scrolling log pane.
    Log(String),
    /// The job mutated on-chain state — refresh the account view now rather
    /// than waiting for the next periodic poll.
    AccountsChanged,
    /// The job finished. `ok` drives the summary's color; `summary` is the
    /// one-line result shown in the log.
    Done { ok: bool, summary: String },
}

/// Sink handed to a job body for streaming progress. Cloneable so a job can
/// pass it into helpers (e.g. a streaming command runner).
#[derive(Clone)]
pub struct Logger {
    tx: Sender<JobEvent>,
}

impl Logger {
    /// Append a line to the log pane.
    pub fn log(&self, msg: impl Into<String>) {
        let _ = self.tx.send(JobEvent::Log(msg.into()));
    }

    /// Signal that on-chain state changed so the loop refreshes promptly.
    pub fn accounts_changed(&self) {
        let _ = self.tx.send(JobEvent::AccountsChanged);
    }
}

/// Spawn `body` on a background thread. Its `Ok(summary)` / `Err` becomes a
/// terminal [`JobEvent::Done`]; `label` names the job in a failure summary.
pub fn spawn<F>(tx: Sender<JobEvent>, label: &'static str, body: F)
where
    F: FnOnce(&Logger) -> anyhow::Result<String> + Send + 'static,
{
    let logger = Logger { tx: tx.clone() };
    thread::spawn(move || {
        let done = match body(&logger) {
            Ok(summary) => JobEvent::Done { ok: true, summary },
            Err(e) => JobEvent::Done {
                ok: false,
                summary: format!("{label} failed: {e:#}"),
            },
        };
        let _ = tx.send(done);
    });
}

/// Run `cmd`, streaming both stdout and stderr into the log line-by-line.
/// Returns `Err` if the process exits non-zero. `label` is the banner shown
/// before the output and named in a failure. Shared by every job that shells
/// out (deploy, the explorer container) so their output interleaves into the
/// log the same way.
pub fn run_streaming(log: &Logger, label: &str, mut cmd: Command) -> Result<()> {
    log.log(format!("$ {label}"));
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("spawn `{label}` (is the toolchain installed?)"))?;

    // stderr on its own reader thread, stdout on this one — both feed the
    // same (cloneable) Logger, interleaving as the process emits them.
    let stderr = child.stderr.take().expect("piped stderr");
    let err_log = log.clone();
    let err_thread = thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            err_log.log(line);
        }
    });
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            log.log(line);
        }
    }
    let _ = err_thread.join();

    let status = child
        .wait()
        .with_context(|| format!("wait for `{label}`"))?;
    if !status.success() {
        bail!("`{label}` exited with {status}");
    }
    Ok(())
}
