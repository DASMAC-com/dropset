//! Wallet resolution shared by both binaries.
//!
//! The wallet is payer, genesis admin, and program upgrade authority — the
//! same key the TUI uses to bootstrap and the teardown script uses to reclaim.
//! Both resolve it the same way: a `--wallet <path>` override (tilde-expanded)
//! or the [`DEFAULT_WALLET`] fallback, read from a keypair file.

use anyhow::{anyhow, Result};
use solana_keypair::Keypair;

/// Default wallet path (mirrors `Anchor.toml`'s `provider.wallet`).
pub const DEFAULT_WALLET: &str = "~/.config/solana/id.json";

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

/// Read the wallet keypair from `arg` (or [`DEFAULT_WALLET`] when `None`),
/// expanding a leading `~/`. Returns the keypair and the resolved path string
/// — the latter is handed to the deploy / `solana program close` CLI calls,
/// which take a filesystem path rather than the in-memory key.
pub fn load(arg: Option<&str>) -> Result<(Keypair, String)> {
    let path = expand_tilde(arg.unwrap_or(DEFAULT_WALLET));
    let wallet = solana_keypair::read_keypair_file(&path)
        .map_err(|e| anyhow!("read wallet keypair {path}: {e}"))?;
    Ok((wallet, path))
}
