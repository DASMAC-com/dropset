//! Wallet resolution shared by both binaries.
//!
//! The wallet is payer, genesis admin, program upgrade authority, and
//! mock-mint authority — the same key the TUI uses to bootstrap and the
//! teardown script uses to reclaim. Both resolve it the same way: a
//! `--wallet <path>` override (tilde-expanded) or the [`DEFAULT_WALLET`]
//! fallback, read from a keypair file. The default is the committed localnet
//! admin keypair, so a localnet run depends on nothing in the operator's
//! Solana store.

use anyhow::{anyhow, Result};
use solana_keypair::Keypair;
use std::path::Path;

/// Default wallet path, relative to the repo root (mirrors `Anchor.toml`'s
/// `provider.wallet`): the committed localnet admin keypair, `keys/BBBB.json`.
pub const DEFAULT_WALLET: &str = "keys/BBBB.json";

/// Expand a leading `~/` to `$HOME`.
fn expand_tilde(path: &str) -> String {
    match path.strip_prefix("~/") {
        Some(rest) => match std::env::var("HOME") {
            Ok(home) => format!("{home}/{rest}"),
            Err(_) => path.to_string(),
        },
        None => path.to_string(),
    }
}

/// Read the wallet keypair from `arg`, or the [`DEFAULT_WALLET`] resolved
/// against `repo_root` when `None`. A user-supplied `arg` is tilde-expanded and
/// otherwise taken as-is; only the default is repo-root-relative, so it lands
/// at an absolute path regardless of the process's working directory. Returns
/// the keypair and the resolved path string — the latter is handed to the
/// deploy / `solana program close` CLI calls, which take a filesystem path
/// rather than the in-memory key.
pub fn load(arg: Option<&str>, repo_root: &Path) -> Result<(Keypair, String)> {
    let path = match arg {
        Some(arg) => expand_tilde(arg),
        None => repo_root
            .join(DEFAULT_WALLET)
            .to_string_lossy()
            .into_owned(),
    };
    let wallet = solana_keypair::read_keypair_file(&path)
        .map_err(|e| anyhow!("read wallet keypair {path}: {e}"))?;
    Ok((wallet, path))
}
