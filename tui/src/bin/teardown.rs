//! `dropset-teardown` — headless rent reclamation.
//!
//! Drives the exact same [`teardown::run`] the TUI's "Teardown & reclaim"
//! action does, but with no UI: discover whatever live accounts exist, drain
//! and close them in the spec's dependency order, and refund all rent to the
//! wallet. The program is left deployed (teardown resets only on-chain state).
//! Built to run in automation or against a real cluster, so unlike the TUI
//! (which is pinned to localnet) it takes an explicit `--rpc-url` and guards
//! any non-localnet target behind an interactive confirmation.
//!
//! ```text
//! dropset-teardown [--wallet <path>] [--rpc-url <url>] [--yes]
//! ```
//!
//! - `--wallet <path>` — admin keypair (payer + registry admin). Defaults to
//!   the Solana CLI wallet.
//! - `--rpc-url <url>` — cluster endpoint. Defaults to the localnet validator
//!   (`http://127.0.0.1:8899`).
//! - `--yes` / `-y` — skip the non-localnet confirmation prompt (for
//!   unattended runs).

// cspell:word rsplit
// cspell:word userinfo

use anyhow::{bail, Result};
use dropset_tui::job::Logger;
use dropset_tui::{chain, teardown, validator, wallet};
use solana_signer::Signer;
use std::io::Write;

fn main() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_help();
        return Ok(());
    }

    let (keypair, _wallet_path) = wallet::load(args.wallet.as_deref())?;
    let rpc_url = args
        .rpc_url
        .unwrap_or_else(|| validator::DEFAULT_RPC_URL.to_string());
    let client = chain::rpc(&rpc_url);

    // A real cluster is irreversible, so make the operator confirm unless they
    // opted out with --yes (or the target is the throwaway localnet).
    if !is_localnet(&rpc_url) && !args.yes {
        confirm(&rpc_url, &keypair.pubkey().to_string())?;
    }

    let log = Logger::stdout();
    let summary = teardown::run(&client, &keypair, &log)?;
    println!("{summary}");
    Ok(())
}

/// Whether `rpc_url` targets the loopback validator — the only target that
/// skips the confirmation prompt. Matches on the URL's **host component**
/// exactly, not a substring: a remote host that merely contains the loopback
/// token (`http://127.0.0.1.evil.com`, `https://127.0.0.1@evil.com`) resolves
/// off-box and must still prompt.
fn is_localnet(rpc_url: &str) -> bool {
    matches!(
        host_of(rpc_url).as_deref(),
        Some("127.0.0.1" | "localhost" | "::1" | "0.0.0.0")
    )
}

/// Best-effort, dependency-free host extraction from a
/// `scheme://[user@]host[:port][/…]` URL, lowercased — enough to classify a
/// teardown target as loopback. `None` when no host is present.
fn host_of(rpc_url: &str) -> Option<String> {
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

/// Block on an interactive `yes` before tearing down a non-localnet cluster.
/// Prints to stderr so a piped stdout (the teardown log) stays clean.
fn confirm(rpc_url: &str, wallet: &str) -> Result<()> {
    eprintln!("⚠  Non-localnet teardown — this is irreversible.");
    eprintln!("   RPC:    {rpc_url}");
    eprintln!("   wallet: {wallet}");
    eprintln!(
        "   Drains and CLOSES every live market, vault, treasury, and the\n   \
         registry, reclaiming all rent to the wallet. The program is left\n   \
         deployed."
    );
    eprint!("   Type 'yes' to continue: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.trim() != "yes" {
        bail!("aborted");
    }
    Ok(())
}

fn print_help() {
    println!(
        "dropset-teardown — headless rent reclamation\n\n\
         USAGE:\n    \
         dropset-teardown [--wallet <path>] [--rpc-url <url>] [--yes]\n\n\
         OPTIONS:\n    \
         -w, --wallet <path>     admin keypair (default: Solana CLI wallet)\n        \
         --rpc-url <url>     cluster endpoint (default: localnet)\n    \
         -y, --yes               skip the non-localnet confirmation prompt\n    \
         -h, --help              show this help"
    );
}

/// Consume the value following an option, rejecting a missing value or a
/// flag-looking token (so `--rpc-url --yes` errors instead of silently taking
/// `--yes` as the URL).
fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    match it.next() {
        Some(v) if !v.starts_with('-') => Ok(v),
        Some(v) => bail!("{flag} needs a value, got flag-like `{v}`"),
        None => bail!("{flag} needs a value"),
    }
}

/// Parsed command line.
struct Args {
    wallet: Option<String>,
    rpc_url: Option<String>,
    yes: bool,
    help: bool,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut a = Args {
            wallet: None,
            rpc_url: None,
            yes: false,
            help: false,
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--wallet" | "-w" => a.wallet = Some(value(&mut it, "--wallet")?),
                "--rpc-url" => a.rpc_url = Some(value(&mut it, "--rpc-url")?),
                "--yes" | "-y" => a.yes = true,
                "--help" | "-h" => a.help = true,
                other => bail!("unknown argument: {other} (try --help)"),
            }
        }
        Ok(a)
    }
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
        // loopback token must still be treated as non-local (and prompt).
        assert!(!is_localnet("http://127.0.0.1.evil.com/"));
        assert!(!is_localnet("http://localhost.attacker.net/"));
        assert!(!is_localnet("https://127.0.0.1@evil.com/"));
        assert!(!is_localnet("http://evil.com/127.0.0.1"));
        assert!(!is_localnet("https://evil.com/?note=localhost"));
        assert!(!is_localnet("https://api.mainnet-beta.solana.com"));
    }

    fn parse(args: &[&str]) -> Result<Args> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn args_parse_flags_and_values() {
        let a = parse(&["--wallet", "/k.json", "--rpc-url", "http://h:1", "-y"]).unwrap();
        assert_eq!(a.wallet.as_deref(), Some("/k.json"));
        assert_eq!(a.rpc_url.as_deref(), Some("http://h:1"));
        assert!(a.yes);
        assert!(!a.help);
    }

    #[test]
    fn args_parse_rejects_unknown_missing_and_flag_value() {
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--wallet"]).is_err()); // missing value
        assert!(parse(&["--rpc-url", "--yes"]).is_err()); // flag swallowed as value
    }
}
