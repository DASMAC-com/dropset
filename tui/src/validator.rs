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

/// Set to `0` to start the localnet on Tower consensus instead of Alpenglow.
const ALPENGLOW_ENV: &str = "DROPSET_LOCALNET_ALPENGLOW";

/// SIMD-0500 ("Disable deployment of SBPF v0, v1 and v2 programs"). A test
/// validator activates every feature it knows at genesis, and Agave 4.3 knows
/// this one, so it would refuse the v0-held program ELF (docs/program-build.md)
/// on deploy. Deactivated until the program builds for sBPF v3.
const SIMD_0500_FEATURE: &str = "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g";

/// Ticks per slot giving mainnet's 200ms slots under Alpenglow (~250ms on the
/// Tower opt-out). The 4.3 test validator paces
/// slots by its tick count (64 ticks, ~400ms) whatever the SIMD-0525 slot-time
/// features say, so the count is halved here. A stopgap: drop it once the
/// standard validator paces 200ms slots on its own.
const TICKS_PER_SLOT: &str = "32";

/// The flags beyond ledger placement: Alpenglow at genesis unless `alpenglow`
/// is off, plus the two stopgaps above. A single node starts Alpenglow from
/// genesis, so this cannot rehearse mainnet's Tower-to-Alpenglow switch.
fn consensus_args(alpenglow: bool) -> Vec<&'static str> {
    let mut args = vec![
        "--deactivate-feature",
        SIMD_0500_FEATURE,
        "--ticks-per-slot",
        TICKS_PER_SLOT,
    ];
    if alpenglow {
        args.push("--alpenglow");
    }
    args
}

/// Whether [`ALPENGLOW_ENV`] leaves Alpenglow on: anything but `0`, unset
/// included.
fn alpenglow_enabled(value: Option<&str>) -> bool {
    value != Some("0")
}

/// Spawn the `solana-test-validator` process against `ledger`, with stdio
/// silenced so its log stream doesn't corrupt the alternate-screen TUI.
fn spawn_child(ledger: &TempDir) -> Result<Child> {
    let alpenglow = alpenglow_enabled(std::env::var(ALPENGLOW_ENV).ok().as_deref());
    Command::new("solana-test-validator")
        .arg("--ledger")
        .arg(ledger.path())
        .arg("--reset")
        .arg("--quiet")
        .args(consensus_args(alpenglow))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawn solana-test-validator (is it on PATH?)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpenglow_is_on_unless_opted_out() {
        assert!(alpenglow_enabled(None));
        assert!(alpenglow_enabled(Some("1")));
        assert!(!alpenglow_enabled(Some("0")));
    }

    #[test]
    fn opt_out_drops_only_alpenglow() {
        let on = consensus_args(true);
        let off = consensus_args(false);
        assert!(on.contains(&"--alpenglow"));
        assert!(!off.contains(&"--alpenglow"));
        for args in [&on, &off] {
            assert!(args
                .windows(2)
                .any(|w| w == ["--deactivate-feature", SIMD_0500_FEATURE]));
            assert!(args
                .windows(2)
                .any(|w| w == ["--ticks-per-slot", TICKS_PER_SLOT]));
        }
    }
}
