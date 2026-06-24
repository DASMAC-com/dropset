//! `dropset-teardown` — headless rent reclamation.
//!
//! Drives the exact same [`teardown::run`] the TUI's "Teardown & reclaim"
//! action does, but with no UI: discover whatever live accounts exist, drain
//! and close them in the spec's dependency order, and refund all rent to the
//! wallet. Built to run in automation or against a real cluster, so unlike the
//! TUI (which is pinned to localnet) it takes an explicit `--rpc-url` and
//! guards any non-localnet target behind an interactive confirmation.
//!
//! ```text
//! dropset-teardown [--wallet <path>] [--rpc-url <url>]
//!                  [--skip-program-close] [--yes]
//! ```
//!
//! - `--wallet <path>` — admin keypair (payer + registry admin + upgrade
//!   authority). Defaults to the Solana CLI wallet.
//! - `--rpc-url <url>` — cluster endpoint. Defaults to the localnet validator
//!   (`http://127.0.0.1:8899`).
//! - `--skip-program-close` — reclaim accounts but leave the deployed program
//!   in place — the sane default on a real cluster.
//! - `--yes` / `-y` — skip the non-localnet confirmation prompt (for
//!   unattended runs).

use anyhow::{anyhow, bail, Result};
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

    let (keypair, wallet_path) = wallet::load(args.wallet.as_deref())?;
    let rpc_url = args
        .rpc_url
        .unwrap_or_else(|| validator::DEFAULT_RPC_URL.to_string());
    let client = chain::rpc(&rpc_url);

    // A real cluster is irreversible, so make the operator confirm unless they
    // opted out with --yes (or the target is the throwaway localnet).
    if !is_localnet(&rpc_url) && !args.yes {
        confirm(
            &rpc_url,
            &keypair.pubkey().to_string(),
            args.skip_program_close,
        )?;
    }

    let log = Logger::stdout();
    let summary = teardown::run(
        &client,
        &keypair,
        &wallet_path,
        &rpc_url,
        args.skip_program_close,
        &log,
    )?;
    println!("{summary}");
    Ok(())
}

/// A URL is "localnet" if it points at the loopback validator — the only
/// target that skips the confirmation prompt.
fn is_localnet(rpc_url: &str) -> bool {
    rpc_url.contains("127.0.0.1") || rpc_url.contains("localhost")
}

/// Block on an interactive `yes` before tearing down a non-localnet cluster.
/// Prints to stderr so a piped stdout (the teardown log) stays clean.
fn confirm(rpc_url: &str, wallet: &str, skip_program_close: bool) -> Result<()> {
    eprintln!("⚠  Non-localnet teardown — this is irreversible.");
    eprintln!("   RPC:    {rpc_url}");
    eprintln!("   wallet: {wallet}");
    eprintln!(
        "   Drains and CLOSES every live market, vault, treasury, and the\n   \
         registry, reclaiming all rent to the wallet."
    );
    if skip_program_close {
        eprintln!("   The deployed program is left in place (--skip-program-close).");
    } else {
        eprintln!(
            "   The deployed program will ALSO be closed — pass\n   \
             --skip-program-close to keep it."
        );
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

fn print_help() {
    println!(
        "dropset-teardown — headless rent reclamation\n\n\
         USAGE:\n    \
         dropset-teardown [--wallet <path>] [--rpc-url <url>] \
         [--skip-program-close] [--yes]\n\n\
         OPTIONS:\n    \
         -w, --wallet <path>     admin keypair (default: Solana CLI wallet)\n        \
         --rpc-url <url>     cluster endpoint (default: localnet)\n        \
         --skip-program-close   reclaim accounts but leave the program deployed\n    \
         -y, --yes               skip the non-localnet confirmation prompt\n    \
         -h, --help              show this help"
    );
}

/// Parsed command line.
struct Args {
    wallet: Option<String>,
    rpc_url: Option<String>,
    skip_program_close: bool,
    yes: bool,
    help: bool,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut a = Args {
            wallet: None,
            rpc_url: None,
            skip_program_close: false,
            yes: false,
            help: false,
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--wallet" | "-w" => {
                    a.wallet = Some(it.next().ok_or_else(|| anyhow!("--wallet needs a path"))?)
                }
                "--rpc-url" => {
                    a.rpc_url = Some(it.next().ok_or_else(|| anyhow!("--rpc-url needs a URL"))?)
                }
                "--skip-program-close" => a.skip_program_close = true,
                "--yes" | "-y" => a.yes = true,
                "--help" | "-h" => a.help = true,
                other => bail!("unknown argument: {other} (try --help)"),
            }
        }
        Ok(a)
    }
}
