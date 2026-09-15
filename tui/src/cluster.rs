//! Which chain the cockpit drives, and the guards that keep a real-funds
//! session from starting by accident.
//!
//! The panel began as a localnet-only tool that spawned its own
//! `solana-test-validator`, so "which cluster" was never a question — the
//! answer was always the throwaway ledger it had just created. Driving real
//! funds makes it a question, and it has to be settled before the first
//! chain call: the localnet bootstrap mints its own tokens and airdrops SOL,
//! neither of which means anything on mainnet, and the ceremony steps move
//! money.
//!
//! Two independent checks, because neither subsumes the other:
//!
//! - [`is_localnet`] classifies the **URL**. Cheap, needs no client, and
//!   available before anything is connected — but a URL only says where the
//!   socket goes. An SSH tunnel or a proxy listening on `localhost:8899` can
//!   forward to mainnet, and the URL cannot tell you that.
//! - [`Cluster::verify_genesis`] asks the **chain** what it is. Authoritative,
//!   because the genesis hash is the chain's permanent identity, but it costs
//!   a round trip and needs a reachable endpoint.
//!
//! Together the URL check picks a policy and the genesis check ratifies it,
//! which matters most in the direction that loses money: a loopback URL that
//! *looks* like a throwaway ledger while actually reaching mainnet is exactly
//! the case the cheap check waves through.
//!
//! **Be precise about where that pairing is actually implemented, because this
//! module owns the vocabulary for it and not every caller applies it.** The
//! headless teardown binary does pair them: it classifies by URL, and where
//! that classification lets it skip the interactive prompt it calls
//! [`ensure_not_mainnet`] before closing anything.
//!
//! The cockpit does **not**. Its policy comes from the operator's declared
//! `--cluster`, so a mainnet session is held to [`Cluster::verify_genesis`]
//! while a localnet one is checked against nothing — the validator it spawns is
//! assumed to be the thing answering on the loopback port. That assumption is
//! older than this module and unchanged by it, but it is an assumption, and
//! stating the pairing as though the cockpit implemented it would be a claim
//! this file cannot support.

use crate::chain;
use crate::validator;
use anyhow::{anyhow, bail, Result};
use solana_client::rpc_client::RpcClient;
use std::io::{IsTerminal, Write};

/// Solana mainnet-beta's genesis hash — the chain's permanent identity, and
/// the only cluster this crate hard-codes. Verified against
/// `api.mainnet-beta.solana.com`'s `getGenesisHash` rather than copied from
/// memory.
pub const MAINNET_GENESIS_HASH: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";

/// Environment variable naming the mainnet JSON-RPC endpoint.
///
/// Deliberately has **no default**. A default would be a URL the operator
/// never typed, which is the one thing a real-funds entry point must not
/// have: the failure mode of a wrong-but-present endpoint is silent, while
/// the failure mode of an absent one is an error message at launch. The
/// public `api.mainnet-beta.solana.com` in particular is rate-limited to the
/// point of being useless for a ceremony, so defaulting to it would trade a
/// clear error for a confusing one.
pub const MAINNET_RPC_ENV: &str = "DROPSET_MAINNET_RPC_URL";

/// The chain a cockpit session drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cluster {
    /// A `solana-test-validator` this process spawns and owns. Throwaway:
    /// mints are mock, SOL is airdropped, and a wipe is free.
    Localnet,
    /// Solana mainnet-beta. Real funds, real mints, nothing reversible.
    Mainnet,
}

impl Cluster {
    /// Parse the `--cluster` value. Accepts `mainnet` and `mainnet-beta` for
    /// the same chain, since both spellings are in common use and rejecting
    /// one would be a pointless trap at a confirm-gated entry point.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "localnet" => Ok(Cluster::Localnet),
            "mainnet" | "mainnet-beta" => Ok(Cluster::Mainnet),
            other => bail!("unknown cluster `{other}` (expected localnet or mainnet)"),
        }
    }

    /// Whether this session drives real funds.
    pub fn is_mainnet(self) -> bool {
        self == Cluster::Mainnet
    }

    /// Display name, for banners and log lines.
    pub fn label(self) -> &'static str {
        match self {
            Cluster::Localnet => "localnet",
            Cluster::Mainnet => "mainnet-beta",
        }
    }

    /// The JSON-RPC endpoint for this cluster.
    ///
    /// Localnet's is a constant because the process spawns that validator
    /// itself, so the URL is never operator-supplied and cannot be wrong.
    /// Mainnet's comes from [`MAINNET_RPC_ENV`] with no fallback — see that
    /// constant for why an absent value is the better failure.
    pub fn rpc_url(self) -> Result<String> {
        match self {
            Cluster::Localnet => Ok(validator::DEFAULT_RPC_URL.to_string()),
            Cluster::Mainnet => match std::env::var(MAINNET_RPC_ENV) {
                Ok(url) if !url.trim().is_empty() => Ok(url.trim().to_string()),
                _ => bail!(
                    "mainnet mode needs {MAINNET_RPC_ENV} set to a JSON-RPC endpoint \
                     (there is deliberately no default — the public endpoint is rate-limited \
                     well below what a ceremony needs)"
                ),
            },
        }
    }

    /// Confirm the endpoint really is the chain this session claims.
    ///
    /// Only meaningful for [`Cluster::Mainnet`]: a test validator mints a
    /// fresh random genesis on every spawn, so there is no localnet hash to
    /// compare against, and the localnet URL is this process's own anyway.
    /// The asymmetry is the point — see [`ensure_not_mainnet`] for the guard
    /// that runs in the other direction, which is where a URL check can be
    /// actively wrong.
    pub fn verify_genesis(self, client: &RpcClient) -> Result<()> {
        if self != Cluster::Mainnet {
            return Ok(());
        }
        // The source error is deliberately DROPPED rather than chained. It comes
        // from the RPC client's HTTP layer, whose `Display` may include the
        // request URL — and this endpoint is the one value in the process most
        // likely to embed an API key. `main` propagates with `?`, so anything
        // kept here is printed to stderr in full on the commonest first-run
        // failure. Fixing it by construction beats reasoning about a
        // dependency's formatting.
        let seen = chain::genesis_hash(client).map_err(|_| {
            anyhow!(
                "could not read the genesis hash from the {} endpoint — is it \
                 reachable? (the underlying error is withheld: it can carry the \
                 endpoint URL, which commonly embeds a key)",
                self.label()
            )
        })?;
        if seen != MAINNET_GENESIS_HASH {
            bail!(
                "{MAINNET_RPC_ENV} does not point at mainnet-beta: genesis {seen}, \
                 expected {MAINNET_GENESIS_HASH}. Refusing to run a mainnet session \
                 against another chain."
            );
        }
        Ok(())
    }
}

/// Refuse to proceed if `client` is talking to mainnet.
///
/// The guard for any path that decided it was safe **from the URL alone** and
/// so skipped an interactive confirmation. Host classification cannot see
/// through a tunnel or a local proxy, so a `localhost` endpoint forwarding to
/// mainnet passes [`is_localnet`] and then does real damage. Asking the chain
/// closes that gap.
///
/// A read failure is *not* treated as mainnet: an unreachable endpoint is the
/// ordinary state of a validator that has not finished booting, and failing
/// closed there would break every localnet run to guard a rare case. The
/// caller's own connection error is the better report.
pub fn ensure_not_mainnet(client: &RpcClient) -> Result<()> {
    match chain::genesis_hash(client) {
        Ok(seen) if seen == MAINNET_GENESIS_HASH => bail!(
            "this endpoint looks local but is MAINNET-BETA (genesis {seen}) — \
             refusing to continue. A tunnel or proxy on a loopback address is \
             the usual cause."
        ),
        _ => Ok(()),
    }
}

/// Whether `rpc_url` targets a validator on **this host**.
///
/// Matches on the URL's **host component** exactly, not a substring: a remote
/// host that merely contains the loopback token
/// (`http://127.0.0.1.evil.com`, `https://127.0.0.1@evil.com`) resolves
/// off-box and must not be classified as local.
///
/// "This host" rather than "loopback" because the accepted set includes
/// `0.0.0.0`, which is the wildcard/unspecified address rather than a loopback
/// one — correct for the intent, since a test validator bound to `0.0.0.0` is
/// still this process's own, but the narrower word would be wrong.
pub fn is_localnet(rpc_url: &str) -> bool {
    matches!(
        host_of(rpc_url).as_deref(),
        Some("127.0.0.1" | "localhost" | "::1" | "0.0.0.0")
    )
}

/// Best-effort, dependency-free host extraction from a
/// `scheme://[user@]host[:port][/…]` URL, lowercased — enough to classify an
/// endpoint as loopback. `None` when no host is present.
pub fn host_of(rpc_url: &str) -> Option<String> {
    let after_scheme = rpc_url.split_once("://").map_or(rpc_url, |(_, rest)| rest);
    // The authority ends at the first '/', '?', or '#'.
    let authority = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
    // Drop any `user[:pass]@` userinfo prefix.
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    // A bracketed IPv6 literal (`[::1]:8899`) keeps its inner colons; an
    // unbracketed host drops its `:port` suffix.
    let host = match host_port.strip_prefix('[') {
        Some(rest) => rest.split_once(']').map_or(rest, |(h, _)| h),
        None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// Block on the operator typing `yes` before continuing.
///
/// Prints to stderr so a piped stdout stays clean, and requires the whole
/// word — a bare `y` is too easy to hit by reflex for a gate whose whole job
/// is to interrupt one.
pub fn confirm() -> Result<()> {
    // A gate whose whole job is to interrupt a reflex must not be satisfiable
    // without a human at a terminal. EOF already failed closed (an empty line
    // is not `yes`), but a pipe, a heredoc, or a verb typed into a terminal tab
    // by automation would all have passed — and this repo does dispatch session
    // verbs programmatically, so that is a live path rather than a theoretical
    // one. Refuse explicitly instead of reading whatever arrives.
    if !std::io::stdin().is_terminal() {
        bail!("refusing to confirm on a non-interactive stdin — run this attended");
    }
    eprint!("   Type 'yes' to continue: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.trim() != "yes" {
        bail!("aborted");
    }
    Ok(())
}

/// Print the real-funds banner and make the operator confirm entry.
///
/// Runs before the alternate screen is entered, so the warning is in the
/// scrollback rather than painted over by the first frame.
pub fn confirm_mainnet_entry(rpc_url: &str, wallet: &str) -> Result<()> {
    let host = host_of(rpc_url).unwrap_or_else(|| "<unparsed>".to_string());
    eprintln!();
    eprintln!("  ╔══════════════════════════════════════════════════════════╗");
    eprintln!("  ║  ⚠  MAINNET-BETA — REAL FUNDS                            ║");
    eprintln!("  ╚══════════════════════════════════════════════════════════╝");
    eprintln!("   RPC host: {host}");
    eprintln!("   wallet:   {wallet}");
    eprintln!(
        "   Genesis verified as mainnet-beta. Every action in this session\n   \
         moves real money and nothing is reversible. No validator is\n   \
         spawned, nothing is deployed, and the localnet bootstrap and wipe\n   \
         are unavailable."
    );
    confirm()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn localnet_matches_loopback_host_exactly() {
        assert!(is_localnet("http://127.0.0.1:8899"));
        assert!(is_localnet("http://localhost:8899"));
        assert!(is_localnet("http://[::1]:8899"));
        assert!(is_localnet("http://0.0.0.0:8899"));
        assert!(is_localnet("https://LOCALHOST")); // case-insensitive host
    }

    #[test]
    fn localnet_rejects_loopback_token_outside_the_host() {
        // The dangerous direction: a remote host that merely contains the
        // loopback token must still be treated as non-local.
        assert!(!is_localnet("http://127.0.0.1.evil.com/"));
        assert!(!is_localnet("http://localhost.attacker.net/"));
        assert!(!is_localnet("https://127.0.0.1@evil.com/"));
        assert!(!is_localnet("http://evil.com/127.0.0.1"));
        assert!(!is_localnet("https://evil.com/?note=localhost"));
        assert!(!is_localnet("https://api.mainnet-beta.solana.com"));
    }

    #[test]
    fn cluster_parses_both_mainnet_spellings_and_rejects_junk() {
        assert_eq!(Cluster::parse("localnet").unwrap(), Cluster::Localnet);
        assert_eq!(Cluster::parse("mainnet").unwrap(), Cluster::Mainnet);
        assert_eq!(Cluster::parse("mainnet-beta").unwrap(), Cluster::Mainnet);
        assert_eq!(Cluster::parse("MAINNET").unwrap(), Cluster::Mainnet);
        assert!(Cluster::parse("devnet").is_err());
        assert!(Cluster::parse("").is_err());
    }

    #[test]
    fn localnet_rpc_url_is_the_spawned_validator() {
        let url = Cluster::Localnet.rpc_url().unwrap();
        assert_eq!(url, validator::DEFAULT_RPC_URL);
        // The localnet URL is this process's own, so it must classify local —
        // the property `verify_genesis` relies on to skip its check.
        assert!(is_localnet(&url));
    }

    #[test]
    fn mainnet_genesis_hash_is_a_plausible_base58_hash() {
        // Guards a truncated or line-wrapped edit of the constant rather than
        // re-asserting the value (which only the chain can confirm).
        assert_eq!(MAINNET_GENESIS_HASH.len(), 44);
        assert!(MAINNET_GENESIS_HASH
            .chars()
            .all(|c| c.is_ascii_alphanumeric()));
        assert!(!MAINNET_GENESIS_HASH.contains(['0', 'O', 'I', 'l']));
    }

    #[test]
    fn localnet_verify_genesis_needs_no_endpoint() {
        // Localnet's check is a no-op by construction, so it must not touch
        // the network: an unreachable URL still succeeds.
        let client = chain::rpc("http://127.0.0.1:1");
        assert!(Cluster::Localnet.verify_genesis(&client).is_ok());
    }

    #[test]
    fn ensure_not_mainnet_passes_when_the_endpoint_is_unreachable() {
        // Fail-open on a read error is deliberate — a booting validator is the
        // common case and must not be reported as mainnet.
        let client = chain::rpc("http://127.0.0.1:1");
        assert!(ensure_not_mainnet(&client).is_ok());
    }
}
