//! The node, sent the terminate signal, prints the shutdown line and exits
//! with success, without waiting for the next heartbeat.

#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn terminate_prints_the_shutdown_line_and_exits_zero() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dropset-node"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn dropset-node");
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));

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
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("run kill");
    assert!(status.success(), "kill -TERM failed: {status}");

    line.clear();
    stdout.read_line(&mut line).expect("read the shutdown line");
    assert_eq!(line.trim_end(), "dropset-node shutting down");

    let exit = child.wait().expect("wait for dropset-node");
    assert!(exit.success(), "dropset-node exited with {exit}");
    // Prompt means well inside the 60 s heartbeat interval.
    assert!(
        sent.elapsed() < Duration::from_secs(10),
        "shutdown took {:?}",
        sent.elapsed()
    );
}
