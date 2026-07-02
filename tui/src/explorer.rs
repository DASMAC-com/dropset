//! Local Solana Explorer, run as a Docker container the TUI supervises.
//!
//! The hosted explorer.solana.com is served from a *public* origin, and
//! modern browsers block a public page from reaching a *loopback* RPC — Brave
//! by default (its localhost-access protection), Safari always (loopback
//! counts as mixed content), and Chromium under Private Network Access (the
//! validator doesn't return `Access-Control-Allow-Private-Network: true`). So
//! the hosted explorer stalls on "loading" against the localnet; it is the
//! browser blocking the fetch, not a CORS or indexer gap (the validator's
//! CORS is fine). Serving the explorer from `http://localhost` makes the page
//! itself loopback, so its client-side fetch to the loopback validator is
//! loopback -> loopback and no browser blocks it.
//!
//! The explorer runs as the seed service of the localnet Docker stack
//! (`infra/localnet/docker-compose.yml`). The TUI owns its lifecycle: built
//! (first run) and started in the background at launch, so it is serving by
//! the time the operator opens it, and torn down on quit — the same ownership
//! the validator has.

use crate::job::{self, Logger};
use anyhow::{bail, Context, Result};
use solana_pubkey::Pubkey;
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Host port the explorer container publishes (compose maps `3100:3000`).
/// Deliberately not `3000`: the frontend's `next dev` (`make frontend`) owns
/// `localhost:3000`, so serving the explorer there too collided — running both
/// left one unreachable. `3100` keeps the local explorer and the frontend up
/// side by side.
pub const EXPLORER_PORT: u16 = 3100;

/// Lifecycle state of the managed explorer container, shared (as an
/// [`AtomicU8`]) between the background starter, the "Open explorer" action,
/// and the UI — so the render loop can read it without blocking on a job.
pub mod state {
    /// Build / start in progress (Docker is present).
    pub const STARTING: u8 = 0;
    /// Serving on [`super::EXPLORER_PORT`].
    pub const READY: u8 = 1;
    /// No Docker CLI — "Open explorer" falls back to the hosted explorer.
    pub const NO_DOCKER: u8 = 2;
    /// Docker is present but the build / start failed.
    pub const FAILED: u8 = 3;
}

/// One-word label for `state`, for the status bar.
pub fn state_label(s: u8) -> &'static str {
    match s {
        state::STARTING => "starting…",
        state::READY => "ready",
        state::NO_DOCKER => "no docker",
        state::FAILED => "failed",
        _ => "?",
    }
}

/// Bring the explorer up on a background thread at TUI launch, recording
/// progress in `status` so it is serving by the time the operator opens it —
/// built lazily the first time, reused after. Serialized via `lock` so it
/// never races the "Open explorer" action's own [`ensure_running`]; streams
/// build output into `log`.
pub fn start_in_background(log: &Logger, repo_root: &Path, status: &AtomicU8, lock: &Mutex<()>) {
    if !docker_available() {
        status.store(state::NO_DOCKER, Ordering::SeqCst);
        log.log("Docker not found — \"Open explorer\" will use the hosted explorer.");
        return;
    }
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    status.store(state::STARTING, Ordering::SeqCst);
    log.log("Starting the local explorer container in the background…");
    match ensure_running(log, repo_root) {
        Ok(()) => {
            status.store(state::READY, Ordering::SeqCst);
            log.log(format!(
                "Local explorer ready on http://localhost:{EXPLORER_PORT}"
            ));
        }
        Err(e) => {
            status.store(state::FAILED, Ordering::SeqCst);
            log.log(format!("Local explorer failed to start: {e:#}"));
        }
    }
}

/// The compose file, relative to the repo root, and the service it defines.
const COMPOSE_REL: &str = "infra/localnet/docker-compose.yml";
const SERVICE: &str = "explorer";

/// Wait this long for the served port after the container starts (`next
/// start` comes up in seconds once the image is built; the build itself is
/// streamed by the `up` command, ahead of this poll).
const READY_TIMEOUT: Duration = Duration::from_secs(90);
const READY_POLL: Duration = Duration::from_millis(500);

/// Build the explorer URL for `address`, served from the local container and
/// pointed at the loopback validator `rpc_url` via the custom-cluster params.
pub fn account_url(address: &Pubkey, rpc_url: &str) -> String {
    format!(
        "http://localhost:{EXPLORER_PORT}/address/{address}?cluster=custom&customUrl={}",
        percent_encode(rpc_url)
    )
}

/// The hosted-explorer URL — the fallback used when Docker isn't available.
/// Won't reach the localnet in Brave/Safari (see the module docs), so callers
/// pair it with a hint.
pub fn hosted_account_url(address: &Pubkey, rpc_url: &str) -> String {
    format!(
        "https://explorer.solana.com/address/{address}?cluster=custom&customUrl={}",
        percent_encode(rpc_url)
    )
}

/// The local-container transaction URL for `signature`, pointed at the loopback
/// validator — the CU pane's per-instruction "latest tx" link.
pub fn tx_url(signature: &str, rpc_url: &str) -> String {
    format!(
        "http://localhost:{EXPLORER_PORT}/tx/{signature}?cluster=custom&customUrl={}",
        percent_encode(rpc_url)
    )
}

/// The hosted-explorer transaction URL — the fallback when Docker isn't
/// available (same browser caveat as [`hosted_account_url`]).
pub fn hosted_tx_url(signature: &str, rpc_url: &str) -> String {
    format!(
        "https://explorer.solana.com/tx/{signature}?cluster=custom&customUrl={}",
        percent_encode(rpc_url)
    )
}

/// Whether a `docker` CLI is on PATH. A `false` steers "Open explorer" to the
/// hosted fallback; a daemon that's installed-but-not-running surfaces later
/// as an `up` failure with docker's own message.
pub fn docker_available() -> bool {
    Command::new("docker")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build (first run) and start the explorer container, then wait for its
/// port. Idempotent — compose reuses the cached image and a running
/// container, so repeat calls are cheap. Output is streamed into `log`.
pub fn ensure_running(log: &Logger, repo_root: &Path) -> Result<()> {
    let compose = repo_root.join(COMPOSE_REL);
    if !compose.exists() {
        bail!("compose file not found at {}", compose.display());
    }
    let mut up = Command::new("docker");
    up.args(["compose", "-f"])
        .arg(&compose)
        .args(["up", "-d", SERVICE])
        .current_dir(repo_root);
    job::run_streaming(log, "docker compose up -d explorer", up)?;
    wait_for_port(log)
}

/// Stop and remove the explorer container. Best-effort: called on quit, so it
/// silences output and only reports a non-zero exit.
pub fn stop(repo_root: &Path) -> Result<()> {
    let compose = repo_root.join(COMPOSE_REL);
    let status = Command::new("docker")
        .args(["compose", "-f"])
        .arg(&compose)
        .arg("down")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run `docker compose down`")?;
    if !status.success() {
        bail!("`docker compose down` exited with {status}");
    }
    Ok(())
}

/// Poll the published port until it accepts a connection or the timeout
/// elapses — enough to know `next start` is serving before we open a browser.
fn wait_for_port(log: &Logger) -> Result<()> {
    log.log(format!(
        "Waiting for the explorer on http://localhost:{EXPLORER_PORT}…"
    ));
    let addr = SocketAddr::from(([127, 0, 0, 1], EXPLORER_PORT));
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if TcpStream::connect_timeout(&addr, READY_POLL).is_ok() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("explorer did not start within {}s", READY_TIMEOUT.as_secs());
        }
        std::thread::sleep(READY_POLL);
    }
}

/// Percent-encode a string for use as a URL query-parameter value. Keeps
/// the RFC 3986 unreserved set (`A–Z a–z 0–9 - _ . ~`) and escapes
/// everything else — enough to encode `http://127.0.0.1:8899` correctly
/// without pulling in a urlencoding crate.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_local_rpc_url() {
        assert_eq!(
            percent_encode("http://127.0.0.1:8899"),
            "http%3A%2F%2F127.0.0.1%3A8899"
        );
    }

    #[test]
    fn local_url_is_served_from_loopback_with_custom_cluster() {
        let addr = Pubkey::new_from_array([1u8; 32]);
        let url = account_url(&addr, "http://127.0.0.1:8899");
        // Served from the local container, not the hosted HTTPS origin — that
        // is the whole point (loopback page -> loopback RPC).
        assert!(url.starts_with("http://localhost:3100/address/"));
        assert!(url.contains("cluster=custom"));
        assert!(url.contains("customUrl=http%3A%2F%2F127.0.0.1%3A8899"));
        assert!(url.contains(&addr.to_string()));
    }

    #[test]
    fn hosted_url_is_the_https_fallback() {
        let addr = Pubkey::new_from_array([2u8; 32]);
        let url = hosted_account_url(&addr, "http://127.0.0.1:8899");
        assert!(url.starts_with("https://explorer.solana.com/address/"));
        assert!(url.contains("customUrl=http%3A%2F%2F127.0.0.1%3A8899"));
    }

    #[test]
    fn tx_urls_target_the_tx_route_on_each_origin() {
        let sig = "5xY5s1gnaturebase58";
        let local = tx_url(sig, "http://127.0.0.1:8899");
        assert!(local.starts_with("http://localhost:3100/tx/5xY5s1gnaturebase58"));
        assert!(local.contains("cluster=custom"));
        assert!(local.contains("customUrl=http%3A%2F%2F127.0.0.1%3A8899"));
        let hosted = hosted_tx_url(sig, "http://127.0.0.1:8899");
        assert!(hosted.starts_with("https://explorer.solana.com/tx/5xY5s1gnaturebase58"));
    }
}
