//! Wallet resolution shared by both binaries.
//!
//! The wallet is payer, genesis admin, program upgrade authority, and
//! mock-mint authority — the same key the TUI uses to bootstrap and the
//! teardown script uses to reclaim. Both resolve it the same way: a
//! `--wallet <path>` override (tilde-expanded) or the [`DEFAULT_WALLET`]
//! fallback, read from a keypair file. The default is the committed localnet
//! admin keypair, so a localnet run depends on nothing in the operator's
//! Solana store.

use anyhow::{anyhow, bail, Result};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
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

/// Read the operator-supplied vault leader keypair (`--leader`), tilde-expanded.
///
/// The parse error is deliberately not echoed: a real-funds key file in an
/// unexpected shape is the one input whose contents must never reach the
/// terminal, and the path plus "not a keypair file" is enough to act on.
pub fn load_leader(arg: &str) -> Result<Keypair> {
    let path = expand_tilde(arg);
    solana_keypair::read_keypair_file(&path).map_err(|_| {
        anyhow!("read leader keypair {path}: not a readable keypair file (contents not shown)")
    })
}

/// The pubkey of every committed keypair under `keys/` — the throwaway
/// localnet role and mint keys, whose **secrets are public** in the repo.
/// A file that does not parse as a keypair is skipped.
pub fn committed_pubkeys(repo_root: &Path) -> Vec<Pubkey> {
    let Ok(entries) = std::fs::read_dir(repo_root.join("keys")) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| solana_keypair::read_keypair_file(&p).ok())
        .map(|k| k.pubkey())
        .collect()
}

/// Refuse `key` in mainnet `role` (`--wallet`, `--leader`) when its secret is
/// committed. Anyone can sign as a committed key, so a vault it led could be
/// withdrawn by anyone and a payer it named could be drained by anyone — on
/// mainnet that is real funds, not a demo.
pub fn refuse_committed(key: &Pubkey, role: &str, committed: &[Pubkey]) -> Result<()> {
    if committed.contains(key) {
        bail!(
            "{role} {key} is a committed keys/ keypair — its secret is public, so it \
             must never hold or control mainnet funds. Pass an operator-held key."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_keys_are_refused_for_mainnet_roles() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let committed = committed_pubkeys(root);
        // The default wallet and the localnet leader are both committed.
        let (admin, _) = load(None, root).unwrap();
        assert!(committed.contains(&admin.pubkey()));
        let leader = load_leader(&root.join("keys/EEEE.json").to_string_lossy()).unwrap();
        assert!(refuse_committed(&leader.pubkey(), "--leader", &committed).is_err());
        // A fresh key is not.
        assert!(refuse_committed(&Keypair::new().pubkey(), "--leader", &committed).is_ok());
    }

    #[test]
    fn a_bad_leader_file_never_echoes_its_contents() {
        let dir = std::env::temp_dir().join(format!("dropset-leader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("leader.json");
        std::fs::write(&path, "\"LeakedSecret\"").unwrap();
        let err = load_leader(&path.to_string_lossy()).unwrap_err();
        let msg = format!("{err:#}");
        std::fs::remove_dir_all(&dir).ok();
        assert!(!msg.contains("LeakedSecret"), "{msg}");
    }
}
