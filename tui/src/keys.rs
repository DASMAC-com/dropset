//! The TUI's mainnet key references — one `op://` reference per signing role,
//! read from a git-ignored env file of the TUI's own (`tui/keys.env`, copied
//! from the committed `tui/keys.env.example`), each overridden by its flag.
//!
//! The file holds references, never secrets, and the TUI parses it itself: it
//! is never `source`d, and the TUI is never run under `op run`, so no resolved
//! secret reaches the process environment. Every reference is resolved in
//! memory through [`wallet::resolve_key`].
//!
//! The leader is the custody key, so it is the one role resolved **lazily**:
//! a [`LazyKey`] holds its reference and runs `op read` only when a custody
//! step first needs the key. A re-centre session, which signs with the quote
//! key alone, never brings the leader into memory.

use crate::cluster::Cluster;
use crate::wallet;
use anyhow::{bail, Context, Result};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;
use std::sync::Mutex;

/// The admin wallet's line in the keys file (`--wallet` overrides it).
pub const WALLET_VAR: &str = "DROPSET_TUI_WALLET";
/// The vault leader's line — the custody key (`--leader` overrides it).
pub const LEADER_VAR: &str = "DROPSET_TUI_LEADER";
/// The vault quote authority's line — the hot-path key (`--quote` overrides it).
pub const QUOTE_VAR: &str = "DROPSET_TUI_QUOTE";

/// The references one keys file names. A role whose line is absent is `None`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyRefs {
    pub wallet: Option<String>,
    pub leader: Option<String>,
    pub quote: Option<String>,
}

impl KeyRefs {
    /// Read and parse the keys file at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read the keys file {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parse the keys file {}", path.display()))
    }

    /// Parse `NAME=value` lines; blank lines and `#` comments are skipped and
    /// a value may be quoted. An unknown name is refused rather than ignored,
    /// so a misspelt role fails at launch instead of reading as absent.
    pub fn parse(text: &str) -> Result<Self> {
        let mut refs = Self::default();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                bail!("line {}: expected NAME=value", n + 1);
            };
            let value = unquote(value.trim());
            let slot = match name.trim() {
                WALLET_VAR => &mut refs.wallet,
                LEADER_VAR => &mut refs.leader,
                QUOTE_VAR => &mut refs.quote,
                other => bail!(
                    "line {}: unknown key `{other}` — expected {WALLET_VAR}, {LEADER_VAR} or {QUOTE_VAR}",
                    n + 1
                ),
            };
            *slot = (!value.is_empty()).then(|| value.to_string());
        }
        Ok(refs)
    }
}

/// `value` without one pair of matching surrounding quotes.
fn unquote(value: &str) -> &str {
    for q in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(q).and_then(|v| v.strip_suffix(q)) {
            return inner;
        }
    }
    value
}

/// An operator key named by reference and resolved on first use, then cached
/// for the session. The checks a launch-time key gets — distinct from the
/// session's other roles, never a committed key on mainnet — run at
/// resolution instead.
pub struct LazyKey {
    reference: String,
    role: &'static str,
    cluster: Cluster,
    /// Keys this one must differ from, each with the role that names it.
    distinct_from: Vec<(Pubkey, &'static str)>,
    committed: Vec<Pubkey>,
    resolved: Mutex<Option<Keypair>>,
}

impl LazyKey {
    pub fn new(
        reference: String,
        role: &'static str,
        cluster: Cluster,
        distinct_from: Vec<(Pubkey, &'static str)>,
        committed: Vec<Pubkey>,
    ) -> Self {
        Self {
            reference,
            role,
            cluster,
            distinct_from,
            committed,
            resolved: Mutex::new(None),
        }
    }

    /// The key, resolving it now if this is its first use.
    pub fn resolve(&self) -> Result<Keypair> {
        let mut slot = self.resolved.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = slot.as_ref() {
            return Ok(key.insecure_clone());
        }
        let key = wallet::resolve_key(&self.reference, self.role, self.cluster)?;
        for (other, role) in &self.distinct_from {
            if key.pubkey() == *other {
                bail!("{} must differ from {role}", self.role);
            }
        }
        if self.cluster.is_mainnet() {
            wallet::refuse_committed(&key.pubkey(), self.role, &self.committed)?;
        }
        *slot = Some(key.insecure_clone());
        Ok(key)
    }

    /// The key's pubkey if it has been resolved — never resolves it.
    pub fn resolved_pubkey(&self) -> Option<Pubkey> {
        let slot = self.resolved.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref().map(Keypair::pubkey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_role_and_skips_comments_and_blanks() {
        let refs = KeyRefs::parse(
            "# keys\n\nDROPSET_TUI_WALLET=op://v/admin/key\n\
             DROPSET_TUI_LEADER = \"op://v/lead/key\"\nDROPSET_TUI_QUOTE='op://v/quote/key'\n",
        )
        .unwrap();
        assert_eq!(refs.wallet.as_deref(), Some("op://v/admin/key"));
        assert_eq!(refs.leader.as_deref(), Some("op://v/lead/key"));
        assert_eq!(refs.quote.as_deref(), Some("op://v/quote/key"));
    }

    #[test]
    fn an_empty_value_reads_as_absent() {
        let refs = KeyRefs::parse("DROPSET_TUI_LEADER=\n").unwrap();
        assert_eq!(refs, KeyRefs::default());
    }

    #[test]
    fn refuses_an_unknown_key_and_a_line_without_equals() {
        let err = KeyRefs::parse("DROPSET_TUI_QOUTE=op://v/q/k").unwrap_err();
        assert!(err.to_string().contains("unknown key `DROPSET_TUI_QOUTE`"));
        assert!(KeyRefs::parse("op://v/q/k").is_err());
    }

    #[test]
    fn the_committed_example_parses() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("keys.env.example");
        let refs = KeyRefs::load(&path).unwrap();
        for value in [refs.wallet, refs.leader, refs.quote] {
            assert!(value.unwrap().starts_with(wallet::OP_REFERENCE_PREFIX));
        }
    }

    fn keypair_file(dir: &Path, name: &str) -> (String, Pubkey) {
        let key = Keypair::new();
        let path = dir.join(name);
        solana_keypair::write_keypair_file(&key, &path).unwrap();
        (path.to_string_lossy().into_owned(), key.pubkey())
    }

    #[test]
    fn a_lazy_key_resolves_once_on_first_use() {
        let dir = std::env::temp_dir().join(format!("lazy-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, pubkey) = keypair_file(&dir, "leader.json");
        let key = LazyKey::new(path.clone(), "--leader", Cluster::Localnet, vec![], vec![]);
        assert_eq!(key.resolved_pubkey(), None);
        assert_eq!(key.resolve().unwrap().pubkey(), pubkey);
        assert_eq!(key.resolved_pubkey(), Some(pubkey));
        // Cached: a second use reads nothing, so it survives the file going.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(key.resolve().unwrap().pubkey(), pubkey);
    }

    #[test]
    fn a_lazy_key_refuses_a_key_it_must_differ_from() {
        let dir = std::env::temp_dir().join(format!("lazy-key-same-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (path, pubkey) = keypair_file(&dir, "leader.json");
        let key = LazyKey::new(
            path,
            "--leader",
            Cluster::Localnet,
            vec![(pubkey, "--quote")],
            vec![],
        );
        let err = key.resolve().unwrap_err().to_string();
        assert!(err.contains("--leader must differ from --quote"), "{err}");
        assert_eq!(key.resolved_pubkey(), None);
    }
}
