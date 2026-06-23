//! `dropset-tui` — a localnet control-plane TUI for the Dropset eCLOB.
//!
//! Spawns a `solana-test-validator`, derives a [`accounts::Phase`] from live
//! on-chain state each refresh, and gates an action menu (deploy → init →
//! create-market → create-vault → teardown → wipe) on it — so the operator
//! can drive and watch the eCLOB end to end. Launched with `make tui`.

mod accounts;
mod action;
mod app;
mod chain;
mod deploy;
mod explorer;
mod job;
mod ui;
mod validator;

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};

/// Default wallet path (mirrors `Anchor.toml`'s `provider.wallet`). Used as
/// payer, genesis admin, and program upgrade authority.
const DEFAULT_WALLET: &str = "~/.config/solana/id.json";

fn main() -> Result<()> {
    let wallet_arg = parse_wallet_arg();
    let wallet_path = expand_tilde(wallet_arg.as_deref().unwrap_or(DEFAULT_WALLET));
    let wallet = solana_keypair::read_keypair_file(&wallet_path)
        .map_err(|e| anyhow!("read wallet keypair {wallet_path}: {e}"))?;
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("locate repo root from crate dir"))?
        .to_path_buf();

    let ctx = action::JobContext {
        rpc_url: validator::DEFAULT_RPC_URL.to_string(),
        repo_root,
        wallet_path,
        wallet,
        explorer_state: Arc::new(AtomicU8::new(explorer::state::STARTING)),
        explorer_lock: Arc::new(Mutex::new(())),
    };
    app::App::new(ctx)?.run()
}

/// Parse `--wallet <path>` / `-w <path>`, or a single positional path.
/// `None` falls back to [`DEFAULT_WALLET`].
fn parse_wallet_arg() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--wallet" | "-w" => return args.next(),
            s if !s.starts_with('-') => return Some(s.to_string()),
            _ => {}
        }
    }
    None
}

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
