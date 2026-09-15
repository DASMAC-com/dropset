//! `dropset-teardown` — headless rent reclamation.
//!
//! Drives the exact same [`teardown::run`] the TUI's "Teardown & reclaim"
//! action does, but with no UI: discover whatever live accounts exist, drain
//! and close them in the spec's dependency order, and refund all rent to the
//! wallet. The program is left deployed (teardown resets only on-chain state).
//! Built to run in automation or against a real cluster, so it takes an
//! explicit `--rpc-url` and guards any non-localnet target behind an
//! interactive confirmation.
//!
//! A loopback `--rpc-url` skips that prompt, so it is additionally held to a
//! genesis check ([`cluster::ensure_not_mainnet`]): host classification cannot
//! see through an SSH tunnel or a local proxy, and this binary CLOSES every
//! market, vault and the registry. That is the one place where the cheap URL
//! check is not merely incomplete but actively wrong, and where being wrong is
//! unrecoverable.
//!
//! ```text
//! dropset-teardown [--wallet <path>] [--rpc-url <url>] [--yes]
//! ```
//!
//! - `--wallet <path>` — admin keypair (payer + registry admin). Defaults to
//!   the committed localnet admin keypair (`keys/BBBB.json`).
//! - `--rpc-url <url>` — cluster endpoint. Defaults to the localnet validator
//!   (`http://127.0.0.1:8899`).
//! - `--yes` / `-y` — skip the non-localnet confirmation prompt (for
//!   unattended runs).

use anyhow::{anyhow, bail, Result};
use dropset_tui::cluster;
use dropset_tui::job::Logger;
use dropset_tui::{chain, teardown, validator, wallet};
use solana_signer::Signer;
use std::path::PathBuf;

fn main() -> Result<()> {
    let args = Args::parse(std::env::args().skip(1))?;
    if args.help {
        print_help();
        return Ok(());
    }

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("locate repo root from crate dir"))?
        .to_path_buf();
    let (keypair, _wallet_path) = wallet::load(args.wallet.as_deref(), &repo_root)?;
    let rpc_url = args
        .rpc_url
        .unwrap_or_else(|| validator::DEFAULT_RPC_URL.to_string());
    let client = chain::rpc(&rpc_url);

    // A real cluster is irreversible, so make the operator confirm unless they
    // opted out with --yes (or the target is the throwaway localnet).
    if cluster::is_localnet(&rpc_url) {
        // The prompt was just skipped on the strength of the URL alone, and a
        // URL cannot see through an SSH tunnel or a local proxy — so ask the
        // chain what it is before closing every market, vault and the registry.
        // This is the one path where the cheap check is not merely incomplete
        // but actively wrong, and the cost of being wrong is unrecoverable.
        cluster::ensure_not_mainnet(&client)?;
    } else if !args.yes {
        confirm(&rpc_url, &keypair.pubkey().to_string())?;
    }

    let log = Logger::stdout();
    let summary = teardown::run(&client, &keypair, &log)?;
    println!("{summary}");
    Ok(())
}

/// Block on an interactive `yes` before tearing down a non-localnet cluster.
/// Prints to stderr so a piped stdout (the teardown log) stays clean; the
/// typed-`yes` read itself is [`cluster::confirm`], shared with the TUI's
/// mainnet entry gate.
fn confirm(rpc_url: &str, wallet: &str) -> Result<()> {
    eprintln!("⚠  Non-localnet teardown — this is irreversible.");
    eprintln!("   RPC:    {rpc_url}");
    eprintln!("   wallet: {wallet}");
    eprintln!(
        "   Drains and CLOSES every live market, vault, treasury, and the\n   \
         registry, reclaiming all rent to the wallet. The program is left\n   \
         deployed."
    );
    cluster::confirm()
}

fn print_help() {
    println!(
        "dropset-teardown — headless rent reclamation\n\n\
         USAGE:\n    \
         dropset-teardown [--wallet <path>] [--rpc-url <url>] [--yes]\n\n\
         OPTIONS:\n    \
         -w, --wallet <path>     admin keypair (default: keys/BBBB.json)\n        \
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
