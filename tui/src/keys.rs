//! The TUI's mainnet keys — one Solana address per signing role, read from a
//! git-ignored env file of the TUI's own (`tui/private-keys.env`, copied from
//! the committed `tui/private-keys.env.example`), each overridden by its flag.
//!
//! The file holds addresses, never secrets, and the TUI parses it itself: it
//! is never `source`d. Each address becomes an [`AgentSigner`] at launch —
//! the SSH agent serving that key signs every transaction, prompting the
//! operator per signature — so the cockpit never holds a mainnet private key.
//! [`agent_key`] refuses an address no agent serves, naming it, before the
//! session starts.

// cspell:word QOUTE

use crate::agent_signer::AgentSigner;
use anyhow::{bail, Context, Result};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

/// The admin wallet's line in the keys file (`--wallet` overrides it).
pub const WALLET_VAR: &str = "DROPSET_TUI_WALLET";
/// The vault leader's line — the custody key (`--leader` overrides it).
pub const LEADER_VAR: &str = "DROPSET_TUI_LEADER";
/// The vault quote authority's line — the hot-path key (`--quote` overrides it).
pub const QUOTE_VAR: &str = "DROPSET_TUI_QUOTE";
/// The program key's line, for a first publish (`--program-key` overrides it).
pub const PROGRAM_KEY_VAR: &str = "DROPSET_TUI_PROGRAM_KEY";

/// One role's signer, shared across job threads: a committed keypair on
/// localnet, an agent-served key on mainnet.
pub type RoleKey = Arc<dyn Signer + Send + Sync>;

/// The addresses one keys file names. A role whose line is absent is `None`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct KeyAddresses {
    pub wallet: Option<String>,
    pub leader: Option<String>,
    pub quote: Option<String>,
    pub program_key: Option<String>,
}

impl KeyAddresses {
    /// Read and parse the keys file at `path`.
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "read the keys file {} — start one by copying tui/private-keys.env.example",
                path.display()
            )
        })?;
        Self::parse(&text).with_context(|| format!("parse the keys file {}", path.display()))
    }

    /// Parse `NAME=value` lines; blank lines and whole-line `#` comments are
    /// skipped and a value may be quoted. Anything ambiguous is refused at
    /// launch rather than read some way that fails later, mid-session: an
    /// unknown or repeated name, and a value carrying an inline comment
    /// (which would otherwise become part of the address).
    pub fn parse(text: &str) -> Result<Self> {
        let mut keys = Self::default();
        let mut seen = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let n = n + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((name, value)) = line.split_once('=') else {
                bail!("line {n}: expected NAME=value");
            };
            let name = name.trim();
            let value = unquote(value.trim());
            let slot = match name {
                WALLET_VAR => &mut keys.wallet,
                LEADER_VAR => &mut keys.leader,
                QUOTE_VAR => &mut keys.quote,
                PROGRAM_KEY_VAR => &mut keys.program_key,
                // Named only when it is shaped like a name: the text before
                // a stray `=` could be the body of a pasted base64 secret.
                other if is_name(other) => bail!(
                    "line {n}: unknown key `{other}` — expected {WALLET_VAR}, {LEADER_VAR}, \
                     {QUOTE_VAR} or {PROGRAM_KEY_VAR}{}",
                    if other.starts_with("export ") {
                        ", with no `export` prefix"
                    } else {
                        ""
                    }
                ),
                _ => bail!("line {n}: expected NAME=value with a known NAME"),
            };
            if seen.contains(&name) {
                bail!("line {n}: {name} is set twice");
            }
            seen.push(name);
            if value.contains(" #") || value.contains("\t#") {
                bail!(
                    "line {n}: {name} carries an inline comment — put comments on their own line"
                );
            }
            *slot = (!value.is_empty()).then(|| value.to_string());
        }
        Ok(keys)
    }
}

/// Whether `s` is shaped like an env-file name — `A-Z`, `0-9` and `_`, after
/// an optional `export ` prefix.
fn is_name(s: &str) -> bool {
    let s = s.strip_prefix("export ").unwrap_or(s).trim_start();
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
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

/// Parse `arg` as the Solana address of mainnet `role`.
///
/// The value is never echoed: a pasted private key is the one wrong input
/// whose text must not reach the terminal, and "not an address" is enough to
/// act on.
pub fn parse_address(arg: &str, role: &str) -> Result<Pubkey> {
    Pubkey::from_str(arg).map_err(|_| {
        anyhow::anyhow!(
            "{role} on mainnet must be a Solana address — the SSH agent holds the key \
             (value not shown)"
        )
    })
}

/// The [`AgentSigner`] for mainnet `role` at address `arg`: the agent at
/// `socket` when given, otherwise the first discovered agent that serves it.
/// Refused, naming the address, when no agent does — the agent config does
/// not list it, or 1Password is locked.
pub fn agent_key(arg: &str, role: &str, socket: Option<&Path>) -> Result<AgentSigner> {
    let address = parse_address(arg, role)?;
    AgentSigner::connect(address, socket).with_context(|| {
        format!(
            "{role} {address} is not served by an SSH agent — list it in the agent's config \
             and unlock 1Password, then relaunch"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> String {
        Pubkey::new_unique().to_string()
    }

    #[test]
    fn parses_each_role_and_skips_comments_and_blanks() {
        let keys = KeyAddresses::parse(
            "# keys\n\nDROPSET_TUI_WALLET=w\nDROPSET_TUI_LEADER = \"l\"\n\
             DROPSET_TUI_QUOTE='q'\nDROPSET_TUI_PROGRAM_KEY=p\n",
        )
        .unwrap();
        assert_eq!(keys.wallet.as_deref(), Some("w"));
        assert_eq!(keys.leader.as_deref(), Some("l"));
        assert_eq!(keys.quote.as_deref(), Some("q"));
        assert_eq!(keys.program_key.as_deref(), Some("p"));
    }

    #[test]
    fn an_empty_value_reads_as_absent() {
        let keys = KeyAddresses::parse("DROPSET_TUI_LEADER=\n").unwrap();
        assert_eq!(keys, KeyAddresses::default());
    }

    #[test]
    fn refuses_an_unknown_key_and_a_line_without_equals() {
        let err = KeyAddresses::parse("DROPSET_TUI_QOUTE=x").unwrap_err();
        assert!(err.to_string().contains("unknown key `DROPSET_TUI_QOUTE`"));
        assert!(KeyAddresses::parse(&address()).is_err());
    }

    #[test]
    fn never_echoes_text_that_is_not_shaped_like_a_name() {
        // A pasted base64 secret splits at its `=` padding; its body must not
        // reach the error.
        let err = KeyAddresses::parse("c2VjcmV0Ym9keQ==")
            .unwrap_err()
            .to_string();
        assert!(!err.contains("c2VjcmV0Ym9keQ"), "{err}");
    }

    #[test]
    fn refuses_a_repeated_name_and_an_inline_comment() {
        let twice = "DROPSET_TUI_QUOTE=a\nDROPSET_TUI_QUOTE=b\n";
        assert!(KeyAddresses::parse(twice)
            .unwrap_err()
            .to_string()
            .contains("set twice"));
        let commented = format!("DROPSET_TUI_LEADER={}  # custody", address());
        let err = KeyAddresses::parse(&commented).unwrap_err().to_string();
        assert!(err.contains("inline comment"), "{err}");
    }

    #[test]
    fn the_committed_example_names_every_role() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("private-keys.env.example");
        let keys = KeyAddresses::load(&path).unwrap();
        for value in [keys.wallet, keys.leader, keys.quote, keys.program_key] {
            assert!(value.unwrap().ends_with("-address>"));
        }
    }

    #[test]
    fn an_address_that_does_not_parse_is_refused_without_its_value() {
        // A 64-byte base58 secret is not a 32-byte address.
        let secret = bs58::encode([7u8; 64]).into_string();
        let err = parse_address(&secret, "--leader").unwrap_err().to_string();
        assert!(err.contains("--leader") && !err.contains(&secret), "{err}");
        let a = address();
        assert_eq!(parse_address(&a, "--leader").unwrap().to_string(), a);
    }

    #[test]
    fn an_address_no_agent_serves_is_refused_by_name() {
        let dead = std::env::temp_dir().join(format!("no-agent-{}.sock", std::process::id()));
        let a = address();
        let err = agent_key(&a, "--quote", Some(&dead)).unwrap_err();
        let text = format!("{err:#}");
        assert!(
            text.contains("--quote") && text.contains(&a) && text.contains("not served"),
            "{text}"
        );
    }
}
