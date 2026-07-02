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
    /// A transaction's measured compute-unit cost, keyed by a short operation
    /// `label` — feeds the CU pane. Re-emitting a label updates its row (units
    /// and the `signature` of the latest transaction, which the pane links to
    /// the explorer).
    Cu {
        label: String,
        units: u64,
        signature: String,
    },
    /// The job finished. `ok` drives the summary's color; `summary` is the
    /// one-line result shown in the log.
    Done { ok: bool, summary: String },
    /// A decoded fill from the program's `emit_cpi!` `FillEvent` subscription
    /// (the [`crate::fills`] thread), tagged with the signature of the swap that
    /// produced it so the fills pane can link each row to the explorer — the
    /// pane keeps a per-market ring of these. Not produced by the single-job
    /// harness.
    Fill {
        signature: String,
        event: dropset_sdk::types::FillEvent,
    },
}

/// Where a [`Logger`] sends its progress. The TUI streams [`JobEvent`]s over
/// the job channel; the headless `dropset-teardown` binary has no UI loop, so
/// it prints to stdout instead. Same `Logger` API drives both — the teardown
/// code path doesn't know or care which sink it's writing to.
#[derive(Clone)]
enum Sink {
    /// Stream events to the TUI event loop.
    Channel(Sender<JobEvent>),
    /// Print log lines to stdout (headless). `accounts_changed` is a no-op —
    /// there's no view to refresh — and `cu` prints a plain line.
    Stdout,
}

/// Sink handed to a job body for streaming progress. Cloneable so a job can
/// pass it into helpers (e.g. a streaming command runner).
#[derive(Clone)]
pub struct Logger {
    sink: Sink,
}

impl Logger {
    /// Build a `Logger` over a job channel — for work that streams into the
    /// log outside the single-job harness (the background explorer starter).
    pub fn new(tx: Sender<JobEvent>) -> Self {
        Self {
            sink: Sink::Channel(tx),
        }
    }

    /// Build a headless `Logger` that prints to stdout — for the standalone
    /// `dropset-teardown` binary, which has no TUI event loop.
    pub fn stdout() -> Self {
        Self { sink: Sink::Stdout }
    }

    /// Append a line to the log pane (or stdout, headless).
    pub fn log(&self, msg: impl Into<String>) {
        match &self.sink {
            Sink::Channel(tx) => {
                let _ = tx.send(JobEvent::Log(msg.into()));
            }
            Sink::Stdout => println!("{}", msg.into()),
        }
    }

    /// Signal that on-chain state changed so the loop refreshes promptly.
    /// A no-op headless — there is no view to refresh.
    pub fn accounts_changed(&self) {
        if let Sink::Channel(tx) = &self.sink {
            let _ = tx.send(JobEvent::AccountsChanged);
        }
    }

    /// Record a transaction's compute-unit cost under `label` for the CU pane
    /// (or a plain stdout line, headless), tagged with the `signature` of the
    /// transaction it measured so the pane can link to it.
    pub fn cu(&self, label: impl Into<String>, units: u64, signature: impl Into<String>) {
        match &self.sink {
            Sink::Channel(tx) => {
                let _ = tx.send(JobEvent::Cu {
                    label: label.into(),
                    units,
                    signature: signature.into(),
                });
            }
            Sink::Stdout => println!("{}: {units} CU", label.into()),
        }
    }
}

/// Spawn `body` on a background thread. Its `Ok(summary)` / `Err` becomes a
/// terminal [`JobEvent::Done`]; `label` names the job in a failure summary.
pub fn spawn<F>(tx: Sender<JobEvent>, label: &'static str, body: F)
where
    F: FnOnce(&Logger) -> anyhow::Result<String> + Send + 'static,
{
    let logger = Logger::new(tx.clone());
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
