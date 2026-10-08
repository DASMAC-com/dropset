//! Which cluster a live run targets, and the guards mainnet mode adds.
//!
//! The maker runs in one of two modes, chosen by `--cluster`:
//!
//! - **Localnet** (the default) — the demo. Quotes the full mock roster against
//!   a test validator, signs with a committed role key, and airdrops its own
//!   fees. Its genesis guard refuses every public cluster.
//! - **Mainnet** — the flash. Quotes only the markets with a real mainnet mint
//!   ([`crate::config::MarketConfig::mainnet_mint`]), and its genesis guard
//!   *requires* mainnet-beta.
//!
//! The assertion runs in both directions on purpose: demo mode refusing mainnet
//! and mainnet mode refusing anything else means no misconfigured `--rpc` can
//! turn one mode into the other. It is keyed on the chain's genesis hash rather
//! than the URL, so a loopback tunnel to a public cluster still trips it
//! (`chain::assert_cluster`).
//!
//! Mainnet mode also changes where the leader key comes from. The committed
//! `keys/` role keys lead nothing real — their secrets are public — so on
//! mainnet the key is resolved from the secrets chain ([`LEADER_SECRET`]:
//! the environment first, then 1Password) into memory only, the `--leader-key`
//! file flag is refused outright, and a key whose pubkey is on the committed
//! roster is refused even when it arrives through the secrets chain. See
//! `docs/key-custody.md` §3.2.

use anyhow::{anyhow, bail, Result};
use dropset_feeds::secrets::SecretProvider;
use solana_keypair::Keypair;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;

/// The canonical `<provider>/<secret>` name of the mainnet leader key — the
/// environment variable `DROPSET_MAKER_LEADER`, or the `maker-leader` field of
/// the `dropset` item in the configured 1Password vault (`feeds::secrets`).
///
/// The value is the keypair in `solana-keygen`'s JSON form: a 64-byte array,
/// secret half first.
pub const LEADER_SECRET: &str = "dropset/maker-leader";

/// Which cluster a live run targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cluster {
    /// A local test validator — the demo.
    Localnet,
    /// Solana mainnet-beta. Real funds; nothing reversible.
    Mainnet,
}

impl Cluster {
    /// Parse a `--cluster` value: `localnet`, or `mainnet` / `mainnet-beta`,
    /// case-insensitively. A copy of `tui/src/cluster.rs`'s `Cluster::parse`
    /// (this crate does not depend on the TUI); change the two together.
    pub fn parse(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "localnet" => Ok(Cluster::Localnet),
            "mainnet" | "mainnet-beta" => Ok(Cluster::Mainnet),
            other => bail!("unknown cluster `{other}` (expected localnet or mainnet)"),
        }
    }

    /// Whether this run targets real funds.
    pub fn is_mainnet(self) -> bool {
        self == Cluster::Mainnet
    }
}

/// The public key of every committed keypair under `keys/` — the localnet
/// role and mock-mint keys, whose **secrets are in the repository**.
///
/// Embedded as public keys rather than read from `keys/` at runtime, for two
/// reasons. A runtime scan is only as good as the working directory, so a
/// maker started anywhere but the repo root would refuse nothing; and the bots
/// image is built with `keys/` excluded from its context, so even
/// `include_str!` of the files would not build there. Only public halves live
/// here — the binary carries no key material. The test
/// `the_committed_roster_matches_keys_dir` holds this list equal to the
/// directory, so a key added there without being added here fails the build's
/// tests rather than slipping past the refusal.
const COMMITTED_PUBKEYS: [Pubkey; 16] = [
    pubkey!("AAAAz3pYUMwhX1bsEtPx9LSWYbpRM8qrFaQgmKVX6oiV"),
    pubkey!("AUDD7hSnRsvgGmhi9spowDhbi7wucwQfE86ut9sAZBmJ"),
    pubkey!("BBBBTc1NfW2YJ4qB98RQuGW5ECssvcwg3DyQ16v1iC5m"),
    pubkey!("CADCCCmJMM9tP7m2AHMnWjTEKA28M5vqyghLD1u6FcNU"),
    pubkey!("CCCC8hQ3P6aotxraFv8Jzv13RhZ9J1UPG4od3EygTVaY"),
    pubkey!("DDDDxRcTmRGx9SZ5BwQnn3ru5i2y27QsCsqaZFZ9T2Yc"),
    pubkey!("EEEE7hMA4awEWjZFJasS9Cw6FroCCUtQLZhLdGEdz7xf"),
    pubkey!("EURCeThrvC3KKDyZEvKSXBgx5aBQBZWkozH3F45CH4rU"),
    pubkey!("FFFFF76hWkT7MZnLb2ZXbrpaLEnQvMjbgamGyvkrPicZ"),
    pubkey!("MXNejRGzxdS4YyJYJyqFPZJeP4WXAfr8n9Je4Vp3Ght"),
    pubkey!("TGBPceijwwpZxVS3HoHWV4iDbryh4eAnUWAgVFSGqHt"),
    pubkey!("USDCqka4GcPP5K2uyVPoNN9Tq6YY8bMrdq3jsjkxUZn"),
    pubkey!("VCHFrwrYZr16LcBEGocuYg8iEqSGyj2cTbJccBo5pVq"),
    pubkey!("XSGDJ6qXNaGujAwBxM6pZfBZ3vBWw81iU3t5ddKHhmw"),
    pubkey!("ZARPfYoPus7NUocgdBz1HCQuZLpL3Y8pzoWQ2WdLJyD"),
    pubkey!("idrxatdM6g4rzL5pWzzHcigyXE9RJa8eHxB9wCj6iwo"),
];

/// Refuse `key` as the mainnet leader when it is on the committed roster.
/// Anyone can sign as a committed key, so a vault it led could be re-priced
/// by anyone. The TUI's mainnet ceremony makes the same refusal
/// (`tui/src/wallet.rs`, `refuse_committed`) from a runtime scan of `keys/`.
fn refuse_committed(key: &Pubkey) -> Result<()> {
    if COMMITTED_PUBKEYS.contains(key) {
        bail!(
            "leader {key} is a committed keys/ keypair — its secret is public, so \
             it must never lead a mainnet vault. Supply an operator-held key \
             through {LEADER_SECRET}."
        );
    }
    Ok(())
}

/// Resolve the leader keypair for `cluster`.
///
/// Localnet reads the `--leader-key` file, or `default_file` when the flag was
/// not passed. Mainnet refuses the flag, resolves [`LEADER_SECRET`] through the
/// secrets chain, and refuses a committed key.
pub fn load_leader(
    cluster: Cluster,
    leader_key_flag: Option<&str>,
    default_file: &str,
) -> Result<Keypair> {
    match cluster {
        Cluster::Localnet => {
            let path = leader_key_flag.unwrap_or(default_file);
            solana_keypair::read_keypair_file(path)
                .map_err(|e| anyhow!("read leader key {path}: {e}"))
        }
        Cluster::Mainnet => {
            if let Some(path) = leader_key_flag {
                bail!(
                    "--leader-key {path} is refused on mainnet: the leader key \
                     never comes from a file there. Supply it through \
                     {LEADER_SECRET} (the environment, or 1Password)."
                );
            }
            let leader = parse_leader_secret(SecretProvider::from_env().resolve(LEADER_SECRET)?)?;
            refuse_committed(&leader.pubkey())?;
            Ok(leader)
        }
    }
}

/// Decode a resolved leader secret — `solana-keygen`'s JSON byte array — and
/// zero this function's own two buffers once the keypair holds the bytes.
///
/// Best-effort hygiene, not a guarantee: copies outside this function — the
/// provider's intermediate strings, the parser's internal buffers, and the
/// `DROPSET_MAKER_LEADER` value in the process environment — are not reached.
///
/// The error never echoes the value or the parser's message, which can quote
/// it: a malformed real-funds secret is the one input whose contents must
/// never reach the terminal.
fn parse_leader_secret(value: String) -> Result<Keypair> {
    let mut raw = value.into_bytes();
    let parsed: Result<Vec<u8>, _> = serde_json::from_slice(&raw);
    raw.fill(0);
    let mut bytes = parsed.map_err(|_| {
        anyhow!("{LEADER_SECRET} is not a solana-keygen JSON byte array (contents not shown)")
    })?;
    let keypair = Keypair::try_from(bytes.as_slice());
    bytes.fill(0);
    keypair
        .map_err(|_| anyhow!("{LEADER_SECRET} does not decode to a keypair (contents not shown)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn cluster_parses_both_mainnet_spellings_and_rejects_junk() {
        assert_eq!(Cluster::parse("localnet").unwrap(), Cluster::Localnet);
        assert_eq!(Cluster::parse("mainnet").unwrap(), Cluster::Mainnet);
        assert_eq!(Cluster::parse("Mainnet-Beta").unwrap(), Cluster::Mainnet);
        assert!(Cluster::parse("devnet").is_err());
    }

    /// The embedded roster is exactly the public halves of `keys/*.json` — no
    /// key missing (it would slip past the refusal) and none stale.
    #[test]
    fn the_committed_roster_matches_keys_dir() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../keys");
        let on_disk: BTreeSet<String> = std::fs::read_dir(dir)
            .expect("read keys/")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .map(|p| {
                solana_keypair::read_keypair_file(&p)
                    .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
                    .pubkey()
                    .to_string()
            })
            .collect();
        let embedded: BTreeSet<String> = COMMITTED_PUBKEYS.iter().map(|k| k.to_string()).collect();
        assert_eq!(embedded.len(), COMMITTED_PUBKEYS.len(), "duplicate entry");
        assert_eq!(embedded, on_disk);
    }

    #[test]
    fn committed_keys_are_refused_and_fresh_ones_pass() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../keys/EEEE.json");
        let localnet_leader = solana_keypair::read_keypair_file(path).unwrap();
        assert!(refuse_committed(&localnet_leader.pubkey()).is_err());
        assert!(refuse_committed(&Keypair::new().pubkey()).is_ok());
    }

    #[test]
    fn the_file_flag_is_refused_on_mainnet_before_any_secret_is_read() {
        let err = load_leader(Cluster::Mainnet, Some("keys/EEEE.json"), "unused")
            .unwrap_err()
            .to_string();
        assert!(err.contains("refused on mainnet"), "{err}");
    }

    #[test]
    fn a_keygen_secret_round_trips() {
        let fresh = Keypair::new();
        let json = serde_json::to_string(&fresh.to_bytes().to_vec()).unwrap();
        assert_eq!(parse_leader_secret(json).unwrap().pubkey(), fresh.pubkey());
    }

    /// A malformed secret is reported by name only — neither the value nor
    /// the parser's message (which can quote it) reaches the error.
    #[test]
    fn a_bad_secret_never_echoes_its_contents() {
        for bad in [
            "not json secret-sentinel",
            "[1, 2, 3]",
            "\"secret-sentinel\"",
        ] {
            let err = format!("{:#}", parse_leader_secret(bad.to_string()).unwrap_err());
            assert!(!err.contains("secret-sentinel"), "{err}");
            assert!(!err.contains("1, 2"), "{err}");
            assert!(err.contains("contents not shown"), "{err}");
        }
    }
}
