//! The Dropset node. It prints one line per stage of its life and nothing
//! else: a start line naming its version and build, a heartbeat once a
//! minute, and a shutdown line on the terminate or interrupt signal, after
//! which it exits with success. See `docs/node.md`.

use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// How long the node waits between heartbeats.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);

/// The line printed once a stop signal arrives, just before a clean exit.
const SHUTDOWN_LINE: &str = "dropset-node shutting down";

/// The git commit the binary was built from, baked in at compile time from
/// `DROPSET_NODE_BUILD` (the Dockerfile passes its `BUILD_COMMIT` argument
/// through). Absent or empty reads as `unknown`.
fn build() -> &'static str {
    build_from(option_env!("DROPSET_NODE_BUILD"))
}

/// The build field for a raw compile-time value, split out of `build` so
/// the `unknown` fallback is testable: `option_env!` is fixed per compile.
fn build_from(raw: Option<&str>) -> &str {
    match raw {
        Some(commit) if !commit.is_empty() => commit,
        _ => "unknown",
    }
}

fn start_line(version: &str, build: &str) -> String {
    format!("dropset-node {version} {build} started")
}

fn heartbeat_line(n: u64) -> String {
    format!("dropset-node heartbeat {n}")
}

fn main() -> ExitCode {
    // Installed before the start line, so a signal sent once that line is
    // visible is always caught rather than killing the process outright.
    let mut signals = match Signals::new([SIGTERM, SIGINT]) {
        Ok(signals) => signals,
        Err(err) => {
            eprintln!("dropset-node: cannot install the signal handler: {err}");
            return ExitCode::FAILURE;
        }
    };
    let (stop_tx, stop_rx) = mpsc::channel();
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            // The receiver outlives this thread, so the send cannot fail.
            let _ = stop_tx.send(());
        }
    });

    println!("{}", start_line(env!("CARGO_PKG_VERSION"), build()));

    // Waiting on the channel, not sleeping, is what makes shutdown prompt: a
    // signal ends the wait at once rather than at the next heartbeat.
    let mut beats: u64 = 0;
    loop {
        match stop_rx.recv_timeout(HEARTBEAT_INTERVAL) {
            Err(RecvTimeoutError::Timeout) => {
                beats += 1;
                println!("{}", heartbeat_line(beats));
            }
            Ok(()) => break,
            // The signal thread ended without a signal. That is not a clean
            // stop, so it must not print the shutdown line or exit 0, which
            // a host reads as one.
            Err(RecvTimeoutError::Disconnected) => {
                eprintln!("dropset-node: the signal thread ended unexpectedly");
                return ExitCode::FAILURE;
            }
        }
    }

    println!("{SHUTDOWN_LINE}");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_line_names_version_and_build() {
        assert_eq!(
            start_line("0.1.0", "abc1234"),
            "dropset-node 0.1.0 abc1234 started"
        );
    }

    #[test]
    fn build_reads_unknown_when_absent_or_empty() {
        assert_eq!(build_from(Some("abc1234")), "abc1234");
        assert_eq!(build_from(Some("")), "unknown");
        assert_eq!(build_from(None), "unknown");
    }

    #[test]
    fn heartbeat_line_carries_its_count() {
        assert_eq!(heartbeat_line(1), "dropset-node heartbeat 1");
        assert_eq!(heartbeat_line(42), "dropset-node heartbeat 42");
    }

    #[test]
    fn shutdown_line_is_fixed() {
        assert_eq!(SHUTDOWN_LINE, "dropset-node shutting down");
    }
}
