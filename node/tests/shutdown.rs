//! The node, sent the terminate signal (a host's stop) or the interrupt
//! signal (Ctrl-C), prints the shutdown line and exits with success, without
//! waiting for the next heartbeat.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Kills the node if an assertion fails first, so a failing run never
/// leaves the process behind. Once the node has exited, the kill is a no-op.
struct Reap(Child);

impl Drop for Reap {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn assert_signal_shuts_down(signal: &str) {
    let child = Command::new(env!("CARGO_BIN_EXE_dropset-node"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn dropset-node");
    let mut node = Reap(child);
    let mut stdout = BufReader::new(node.0.stdout.take().expect("piped stdout"));

    // The handler is installed before the start line prints, so once the
    // line is read the signal is caught rather than killing the process.
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read the start line");
    assert!(
        line.starts_with("dropset-node ") && line.trim_end().ends_with(" started"),
        "unexpected start line: {line:?}"
    );

    let sent = Instant::now();
    let status = Command::new("kill")
        .args([signal, &node.0.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill {signal} failed: {status}");

    line.clear();
    stdout.read_line(&mut line).expect("read the shutdown line");
    assert_eq!(
        line.trim_end(),
        "dropset-node shutting down",
        "after {signal}"
    );

    let exit = node.0.wait().expect("wait for dropset-node");
    assert!(
        exit.success(),
        "dropset-node exited with {exit} after {signal}"
    );
    // Prompt means well inside the 60 s heartbeat interval.
    assert!(
        sent.elapsed() < Duration::from_secs(10),
        "shutdown after {signal} took {:?}",
        sent.elapsed()
    );
}

#[test]
fn terminate_prints_the_shutdown_line_and_exits_zero() {
    assert_signal_shuts_down("-TERM");
}

#[test]
fn interrupt_prints_the_shutdown_line_and_exits_zero() {
    assert_signal_shuts_down("-INT");
}
