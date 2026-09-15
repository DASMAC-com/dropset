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
//! (`infra/localnet/docker-compose.yml`). The TUI owns its lifecycle: its
//! image is pulled from Docker Hub (built from source only as a fallback) and
//! the container started in the background at launch, so it is serving by the
//! time the operator opens it, and torn down on quit — the same ownership the
//! validator has.

use crate::cluster::Cluster;
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
/// pulled (or built) the first time, reused after. Serialized via `lock` so it
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

/// The hosted-explorer URL for `address` on **mainnet-beta**.
///
/// Carries no `customUrl` — and that omission is the point, not an economy.
/// The mainnet endpoint routinely embeds an API key in its URL (that is how
/// most paid providers authenticate), and the `customUrl` form would paste it
/// into a query string that lands in the browser's history, its address bar,
/// and any referrer the explorer sends onward. The public explorer already
/// defaults to mainnet-beta, so the parameter buys nothing and leaks a
/// credential.
pub fn mainnet_account_url(address: &Pubkey) -> String {
    format!("https://explorer.solana.com/address/{address}")
}

/// The hosted-explorer transaction URL on mainnet-beta — same
/// no-`customUrl` rule as [`mainnet_account_url`], for the same reason.
pub fn mainnet_tx_url(signature: &str) -> String {
    format!("https://explorer.solana.com/tx/{signature}")
}

/// The account URL to open, given the cluster and whether the local container
/// is serving.
///
/// **One owner for the rule, because there were two.** `App::open_in_explorer`
/// and `action::dispatch`'s `OpenExplorer` arm each chose a builder
/// independently, and when mainnet arrived only the first learned about it — so
/// the single action mainnet exposes went on percent-encoding the endpoint into
/// a `customUrl=` parameter. A caller now *asks* which URL to use rather than
/// choosing, which is the same reason `App::run_action` is the one choke point
/// for the availability gates.
pub fn account_url_for(
    cluster: Cluster,
    address: &Pubkey,
    rpc_url: &str,
    local_ready: bool,
) -> String {
    match cluster {
        Cluster::Mainnet => mainnet_account_url(address),
        Cluster::Localnet if local_ready => account_url(address, rpc_url),
        Cluster::Localnet => hosted_account_url(address, rpc_url),
    }
}

/// The transaction URL to open — same rule and same reasoning as
/// [`account_url_for`].
pub fn tx_url_for(cluster: Cluster, signature: &str, rpc_url: &str, local_ready: bool) -> String {
    match cluster {
        Cluster::Mainnet => mainnet_tx_url(signature),
        Cluster::Localnet if local_ready => tx_url(signature, rpc_url),
        Cluster::Localnet => hosted_tx_url(signature, rpc_url),
    }
}

/// The lifecycle state a fresh session starts its explorer tracking in.
///
/// Mainnet starts at [`state::NO_DOCKER`] rather than [`state::STARTING`], and
/// that is load-bearing rather than cosmetic: `App`'s `Drop` tears the managed
/// container down unless the state is `NO_DOCKER`, so a session that never
/// brings one up must not claim one. Extracted from the call site so it is
/// testable — deleting the distinction used to break nothing.
pub fn initial_state(cluster: Cluster) -> u8 {
    match cluster {
        Cluster::Localnet => state::STARTING,
        Cluster::Mainnet => state::NO_DOCKER,
    }
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

/// Start the explorer container, then wait for its port. Idempotent — the
/// first call pulls the published image (or builds from source as a fallback)
/// and later calls reuse the cached image and a running container, so repeat
/// calls are cheap. Output is streamed into `log`.
pub fn ensure_running(log: &Logger, repo_root: &Path) -> Result<()> {
    let compose = repo_root.join(COMPOSE_REL);
    if !compose.exists() {
        bail!("compose file not found at {}", compose.display());
    }
    // `up` resolves the image itself from the compose file's `image` + `build`
    // + `pull_policy: missing`: a locally-cached image is used as-is (so a
    // repeat launch, and an offline one, is instant and needs no registry),
    // an absent one is pulled from Docker Hub, and only a failed pull falls
    // back to the from-source build. So the common path pulls rather than
    // builds without any explicit pull/build orchestration here.
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
///
/// Scoped to the explorer service, **not** a project-wide `docker compose
/// down`. The compose project now holds the shared `dropset` Postgres and the
/// market-data collectors, so tearing the whole project down on quit would stop
/// a collector mid-backfill — and it is quit, so nothing would explain why.
pub fn stop(repo_root: &Path) -> Result<()> {
    let compose = repo_root.join(COMPOSE_REL);
    let status = Command::new("docker")
        .args(["compose", "-f"])
        .arg(&compose)
        .args(["rm", "-sf", SERVICE])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("run `docker compose rm -sf explorer`")?;
    if !status.success() {
        bail!("`docker compose rm -sf explorer` exited with {status}");
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
        let sig = "5xY5s1Vd7z9Kq2Rp8";
        let local = tx_url(sig, "http://127.0.0.1:8899");
        assert!(local.starts_with("http://localhost:3100/tx/5xY5s1Vd7z9Kq2Rp8"));
        assert!(local.contains("cluster=custom"));
        assert!(local.contains("customUrl=http%3A%2F%2F127.0.0.1%3A8899"));
        let hosted = hosted_tx_url(sig, "http://127.0.0.1:8899");
        assert!(hosted.starts_with("https://explorer.solana.com/tx/5xY5s1Vd7z9Kq2Rp8"));
    }

    #[test]
    fn mainnet_builders_emit_no_query_string() {
        let addr = Pubkey::new_from_array([3u8; 32]);
        let url = mainnet_account_url(&addr);
        assert_eq!(url, format!("https://explorer.solana.com/address/{addr}"));
        assert!(!url.contains('?'));
        let tx = mainnet_tx_url("5xY5s1Vd7z9Kq2Rp8");
        assert_eq!(tx, "https://explorer.solana.com/tx/5xY5s1Vd7z9Kq2Rp8");
        assert!(!tx.contains('?'));
    }

    #[test]
    fn mainnet_routing_never_reaches_an_endpoint_bearing_url() {
        // This assertion is deliberately at the ROUTING layer rather than on
        // the builders. The builders cannot leak by construction — they take no
        // endpoint at all — so asserting that their output omits one proves
        // nothing their signature does not already guarantee. What actually
        // regressed is a *caller* reaching for the localnet builder on mainnet,
        // which no test of a builder alone can see.
        let addr = Pubkey::new_from_array([4u8; 32]);
        let keyed = "https://mainnet.example.com/?api-key=SUPERSECRET";

        for local_ready in [true, false] {
            let url = account_url_for(Cluster::Mainnet, &addr, keyed, local_ready);
            assert!(!url.contains("SUPERSECRET"), "leaked the key: {url}");
            assert!(!url.contains("customUrl"), "leaked the endpoint: {url}");
            assert_eq!(url, mainnet_account_url(&addr));

            let tx = tx_url_for(Cluster::Mainnet, "5xY5s1Vd7z9Kq2Rp8", keyed, local_ready);
            assert!(!tx.contains("SUPERSECRET"), "leaked the key: {tx}");
            assert!(!tx.contains("customUrl"), "leaked the endpoint: {tx}");
            assert_eq!(tx, mainnet_tx_url("5xY5s1Vd7z9Kq2Rp8"));
        }

        // The contrast is what makes the assertions above falsifiable: the
        // localnet builders DO embed the endpoint, deliberately. Both localnet
        // branches embed it, so neither is a safe accidental fallback.
        assert!(account_url_for(Cluster::Localnet, &addr, keyed, true).contains("SUPERSECRET"));
        assert!(account_url_for(Cluster::Localnet, &addr, keyed, false).contains("SUPERSECRET"));
        assert!(tx_url_for(Cluster::Localnet, "sig", keyed, true).contains("SUPERSECRET"));
        assert!(tx_url_for(Cluster::Localnet, "sig", keyed, false).contains("SUPERSECRET"));
    }

    #[test]
    fn mainnet_starts_its_explorer_tracking_as_no_docker() {
        // Load-bearing, not cosmetic: `Drop for App` tears the managed
        // container down unless the state is NO_DOCKER, so a mainnet session
        // (which never starts one) must not claim one. Deleting the
        // distinction used to break no test at all.
        assert_eq!(initial_state(Cluster::Mainnet), state::NO_DOCKER);
        assert_eq!(initial_state(Cluster::Localnet), state::STARTING);
        assert_ne!(state::NO_DOCKER, state::STARTING);
    }
}
