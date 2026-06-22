//! `solana-test-validator` child-process management.
//!
//! The ledger lives in a [`TempDir`] under the system temp dir — never in
//! the repo — so a wipe is a clean `remove_dir_all` (handled by `TempDir`'s
//! `Drop`) and chain state can't bloat the working tree. The child is
//! killed and reaped on `Drop`, so quitting the TUI (or a panic unwinding
//! through it) leaves no orphaned validator and no leftover ledger.

use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};
use tempfile::TempDir;

/// Default JSON-RPC endpoint a freshly-spawned `solana-test-validator`
/// listens on.
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";

/// A running `solana-test-validator` and its temp-dir ledger.
pub struct Validator {
    child: Child,
    /// Held for its `Drop` — removing the temp ledger. Never read.
    _ledger: TempDir,
    rpc_url: String,
}

impl Validator {
    /// Spawn a validator with a fresh temp-dir ledger. Returns once the
    /// process is launched; readiness is observed out-of-band by polling
    /// RPC (the derived `Phase` stays `NoValidator` until `get_slot`
    /// succeeds), so this does not block on boot.
    pub fn spawn() -> Result<Self> {
        let ledger = tempfile::Builder::new()
            .prefix("dropset-localnet-")
            .tempdir()
            .context("create temp ledger dir")?;
        let child = spawn_child(&ledger)?;
        Ok(Self {
            child,
            _ledger: ledger,
            rpc_url: DEFAULT_RPC_URL.to_string(),
        })
    }

    /// The validator's JSON-RPC URL.
    pub fn rpc_url(&self) -> &str {
        &self.rpc_url
    }

    /// Kill the validator, wipe its ledger, and spawn a fresh one in a new
    /// temp dir — a clean-slate restart. The old ledger's `TempDir` is
    /// dropped (removed) once the new one is in place.
    pub fn wipe_and_respawn(&mut self) -> Result<()> {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let ledger = tempfile::Builder::new()
            .prefix("dropset-localnet-")
            .tempdir()
            .context("create temp ledger dir")?;
        self.child = spawn_child(&ledger)?;
        self._ledger = ledger;
        Ok(())
    }
}

impl Drop for Validator {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        // `_ledger` (TempDir) is removed by its own Drop right after this.
    }
}

/// Spawn the `solana-test-validator` process against `ledger`, with stdio
/// silenced so its log stream doesn't corrupt the alternate-screen TUI.
fn spawn_child(ledger: &TempDir) -> Result<Child> {
    Command::new("solana-test-validator")
        .arg("--ledger")
        .arg(ledger.path())
        .arg("--reset")
        .arg("--quiet")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn solana-test-validator (is it on PATH?)")
}
