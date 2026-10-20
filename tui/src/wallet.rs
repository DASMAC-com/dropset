//! Wallet resolution shared by both binaries.
//!
//! The wallet is payer, genesis admin, program upgrade authority, and
//! mock-mint authority — the same key the TUI uses to bootstrap and the
//! teardown script uses to reclaim. Both resolve it the same way: a
//! `--wallet <path>` override (tilde-expanded) or the [`DEFAULT_WALLET`]
//! fallback, read from a keypair file. The default is the committed localnet
//! admin keypair, so a localnet run depends on nothing in the operator's
//! Solana store.
//!
//! Mainnet keys never come from a file. Every mainnet role — admin, leader,
//! quote authority, program — is named by a 1Password secret reference
//! (`op://vault/item/field`) and resolved by [`resolve_key`] through the `op`
//! CLI into memory — at launch, except the leader, which resolves on its first
//! custody step ([`crate::keys`]) — so the key exists on disk nowhere this
//! process controls and the code that drives the ceremony never holds it as
//! text.

use crate::cluster::Cluster;
use anyhow::{anyhow, bail, Context, Result};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;
use std::process::{Command, Stdio};

/// The prefix of a 1Password secret reference.
pub const OP_REFERENCE_PREFIX: &str = "op://";

/// The `--wallet` value that asks for a fresh in-memory keypair, generated at
/// launch and never written anywhere — an unfunded identity for a session that
/// only watches (`make mainnet-demo`). Refused for every other role.
pub const EPHEMERAL_WALLET: &str = "ephemeral";

/// Optional environment variable naming the 1Password account `op` reads
/// from, passed as `--account` — the same explicit-account pattern the
/// session secrets use, so a machine signed into several accounts never
/// resolves against whichever one happens to be the default.
pub const OP_ACCOUNT_ENV: &str = "DROPSET_OP_ACCOUNT";

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

/// Resolve the keypair for a mainnet `role` (`--wallet`, `--leader`,
/// `--program-key`) from `arg`.
///
/// On mainnet only a 1Password reference is accepted (or, for a watch-only
/// session, [`EPHEMERAL_WALLET`]); a file path is refused outright rather than
/// read, because a key file is the custody anti-pattern the reference exists to
/// replace. On localnet a reference is refused the other way — the localnet
/// roles are the committed keys, and the deploy and close paths there shell out
/// with a file path — so each cluster has exactly one key source.
pub fn resolve_key(arg: &str, role: &str, cluster: Cluster) -> Result<Keypair> {
    let is_reference = arg.starts_with(OP_REFERENCE_PREFIX);
    match cluster {
        Cluster::Mainnet if is_reference => op_read_keypair("op", arg, role),
        // Watch-only admin only. A throwaway leader would let the admin-signed
        // create-vault open a real vault nobody can ever lead again.
        Cluster::Mainnet if arg == EPHEMERAL_WALLET && role == "--wallet" => Ok(Keypair::new()),
        Cluster::Mainnet if arg == EPHEMERAL_WALLET => bail!(
            "{role} cannot be `{EPHEMERAL_WALLET}` — only a watch-only --wallet may be thrown away"
        ),
        Cluster::Mainnet => bail!(
            "{role} on mainnet must be a 1Password reference ({OP_REFERENCE_PREFIX}vault/item/field) \
             — key files are refused on a real cluster"
        ),
        Cluster::Localnet if is_reference => bail!(
            "{role} on localnet is a keypair file — 1Password references are for mainnet"
        ),
        Cluster::Localnet => load_leader(arg),
    }
}

/// Read `reference` with `op read` (the binary at `program`) and decode it as a
/// keypair.
///
/// The secret travels from `op`'s stdout straight into the decoder and the
/// buffers are overwritten afterwards — best effort, since copies made while
/// the output was read are not reached — and nothing about it is ever
/// formatted into an error. `op`'s own stderr is passed through on failure — it names the
/// reference and the reason (signed out, no such item), never the value.
fn op_read_keypair(program: &str, reference: &str, role: &str) -> Result<Keypair> {
    let mut cmd = Command::new(program);
    cmd.arg("read").arg("--no-newline");
    if let Some(account) = std::env::var(OP_ACCOUNT_ENV)
        .ok()
        .filter(|a| !a.trim().is_empty())
    {
        cmd.arg("--account").arg(account);
    }
    let out = cmd
        .arg(reference)
        .stdin(Stdio::null())
        .output()
        .with_context(|| {
            format!("run `{program} read` for {role} (is the 1Password CLI installed?)")
        })?;
    let mut secret = out.stdout;
    if !out.status.success() {
        secret.fill(0);
        bail!(
            "{role}: `op read {reference}` failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let decoded = decode_keypair(&secret);
    secret.fill(0);
    decoded.map_err(|why| {
        anyhow!("{role}: {reference} is not a Solana keypair ({why}; contents not shown)")
    })
}

/// Decode a stored keypair in any form an operator would paste into a
/// 1Password field: the `solana-keygen` JSON byte array, the 128-character
/// hex of those 64 bytes (secret then public — how the program key is
/// stored), or the base58 string wallets export. The error names the shape
/// that failed, never the bytes.
fn decode_keypair(raw: &[u8]) -> std::result::Result<Keypair, &'static str> {
    let text = std::str::from_utf8(raw).map_err(|_| "not UTF-8")?.trim();
    let is_hex = text.len() == 128 && text.bytes().all(|b| b.is_ascii_hexdigit());
    let mut bytes = if is_hex {
        (0..64)
            .map(|i| u8::from_str_radix(&text[2 * i..2 * i + 2], 16))
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| "malformed hex")?
    } else if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        inner
            .split(',')
            .map(|b| b.trim().parse::<u8>())
            .collect::<std::result::Result<Vec<u8>, _>>()
            .map_err(|_| "a JSON array with a non-byte entry")?
    } else {
        bs58::decode(text)
            .into_vec()
            .map_err(|_| "neither a JSON byte array nor base58")?
    };
    let keypair = Keypair::try_from(bytes.as_slice()).map_err(|_| "not a 64-byte keypair");
    bytes.fill(0);
    keypair
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

    #[test]
    fn each_cluster_takes_exactly_one_key_source() {
        let mainnet_file = resolve_key("/k.json", "--wallet", Cluster::Mainnet).unwrap_err();
        assert!(format!("{mainnet_file:#}").contains("1Password"));
        let localnet_ref = resolve_key("op://v/i/f", "--wallet", Cluster::Localnet).unwrap_err();
        assert!(format!("{localnet_ref:#}").contains("localnet"));
        // Watch-only mainnet sessions get a fresh key, never a file.
        let a = resolve_key(EPHEMERAL_WALLET, "--wallet", Cluster::Mainnet).unwrap();
        let b = resolve_key(EPHEMERAL_WALLET, "--wallet", Cluster::Mainnet).unwrap();
        assert_ne!(a.pubkey(), b.pubkey());
        // A role that signs or leads something durable never is.
        for role in ["--leader", "--program-key"] {
            assert!(
                resolve_key(EPHEMERAL_WALLET, role, Cluster::Mainnet).is_err(),
                "{role}"
            );
        }
    }

    #[test]
    fn decodes_both_stored_keypair_shapes() {
        let kp = Keypair::new();
        let json = format!("[{}]\n", kp.to_bytes().map(|b| b.to_string()).join(","));
        assert_eq!(
            decode_keypair(json.as_bytes()).unwrap().pubkey(),
            kp.pubkey()
        );
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        assert_eq!(
            decode_keypair(b58.as_bytes()).unwrap().pubkey(),
            kp.pubkey()
        );
        let hex: String = kp.to_bytes().iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            decode_keypair(hex.as_bytes()).unwrap().pubkey(),
            kp.pubkey()
        );
        let upper = hex.to_uppercase();
        assert_eq!(
            decode_keypair(upper.as_bytes()).unwrap().pubkey(),
            kp.pubkey()
        );
        // A public half that does not match its secret half is refused.
        let mut forged = kp.to_bytes();
        forged[40] ^= 1;
        let forged: String = forged.iter().map(|b| format!("{b:02x}")).collect();
        assert!(decode_keypair(forged.as_bytes()).is_err());
        assert!(decode_keypair(b"[1,2,3]").is_err());
        assert!(decode_keypair(b"[1,2,999]").is_err());
        assert!(decode_keypair(b"not-a-key!").is_err());
    }

    /// A stand-in `op` that prints `stdout` and exits `code`, in its own dir.
    #[cfg(unix)]
    fn fake_op(tag: &str, stdout: &str, code: i32) -> (std::path::PathBuf, String) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("dropset-op-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("op");
        let script =
            format!("#!/bin/sh\nprintf '%s' '{stdout}'\necho 'item not found' >&2\nexit {code}\n");
        std::fs::write(&bin, script).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
        let program = bin.to_string_lossy().into_owned();
        (dir, program)
    }

    #[cfg(unix)]
    #[test]
    fn op_read_resolves_in_memory_and_never_echoes_the_value() {
        let kp = Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let (dir, op) = fake_op("ok", &b58, 0);
        let got = op_read_keypair(&op, "op://v/admin/key", "--wallet").unwrap();
        assert_eq!(got.pubkey(), kp.pubkey());
        std::fs::remove_dir_all(&dir).ok();

        // A value that is not a keypair is refused without echoing it.
        let (dir, op) = fake_op("bad", "LeakedSecret", 0);
        let msg = format!(
            "{:#}",
            op_read_keypair(&op, "op://v/i/f", "--leader").unwrap_err()
        );
        assert!(
            !msg.contains("LeakedSecret") && msg.contains("op://v/i/f"),
            "{msg}"
        );
        std::fs::remove_dir_all(&dir).ok();

        // A failed read reports op's stderr — the reason — and not its stdout.
        let (dir, op) = fake_op("fail", "LeakedSecret", 1);
        let msg = format!(
            "{:#}",
            op_read_keypair(&op, "op://v/i/f", "--leader").unwrap_err()
        );
        assert!(
            !msg.contains("LeakedSecret") && msg.contains("item not found"),
            "{msg}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
