//! `dropset-tui` — the control-plane TUI for the Dropset eCLOB.
//!
//! On **localnet** (the default) it spawns a `solana-test-validator`, derives
//! a [`accounts::Phase`] from live on-chain state each refresh, and gates an
//! action menu (deploy → init → create-market → create-vault → teardown →
//! wipe) on it — so the operator can drive and watch the eCLOB end to end.
//! Launched with `make tui`.
//!
//! On **mainnet** (`--cluster mainnet`, `make tui-mainnet`) it is the same
//! panel pointed at a chain it does not own: no validator is spawned, nothing
//! is deployed, and the localnet bootstrap and wipe are unavailable. Entry is
//! guarded three ways, each catching something the others cannot — the
//! endpoint must be named explicitly by environment variable, its genesis hash
//! must actually be mainnet-beta's, and the operator must type `yes` to a
//! real-funds banner.
//!
//! On mainnet the menu offers the one-shot ceremony — init, create-market,
//! create-vault, deposit — and nothing else (see
//! [`action::Action::available_on`]). Each step re-checks the chain and
//! refuses when its account already exists, and references the real roster
//! mints rather than minting any. The vault steps sign with the leader the
//! operator names with `--leader`; the admin is `--wallet`.
//!
//! ```text
//! dropset-tui [--cluster <localnet|mainnet>] [--wallet <path>] [--leader <path>] [--bootstrap]
//! ```

use anyhow::{anyhow, bail, Result};
use dropset_tui::cluster::{self, Cluster};
use dropset_tui::{action, app, chain, explorer, wallet};
use solana_signer::Signer;
use std::path::PathBuf;
use std::sync::atomic::AtomicU8;
use std::sync::{Arc, Mutex};

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
    let (wallet, wallet_path) = wallet::load(args.wallet.as_deref(), &repo_root)?;

    // Refuse rather than ignore. Silently dropping the flag would leave the
    // operator believing the turnkey path ran, and "nothing happened" is
    // indistinguishable from "it worked" until the book is inspected.
    if args.bootstrap && args.cluster.is_mainnet() {
        bail!(
            "--bootstrap is a localnet-only turnkey path (it deploys the program \
             and mints mock tokens) and has no mainnet meaning"
        );
    }

    // Localnet's URL is this process's own spawned validator; mainnet's must be
    // named explicitly, and is then held to the genesis hash before the
    // operator is asked to confirm anything — so the banner describes a chain
    // that has already been identified, not one that is merely configured.
    // The operator's vault leader. Mainnet-only, and refused on localnet for
    // the same reason as `--bootstrap`: the localnet roster leads with its
    // committed role key, so a supplied one would be silently ignored.
    let leader = match &args.leader {
        None => None,
        Some(_) if !args.cluster.is_mainnet() => bail!(
            "--leader is mainnet-only — the localnet roster leads with its committed role key"
        ),
        Some(path) => {
            let kp = wallet::load_leader(path)?;
            if kp.pubkey() == wallet.pubkey() {
                bail!("--leader must differ from --wallet: the program rejects one key as both admin and leader");
            }
            Some(kp)
        }
    };

    // A committed keypair's secret is public, so on mainnet neither role may
    // be one. This also makes `--wallet` mandatory there: the default is the
    // committed localnet admin.
    if args.cluster.is_mainnet() {
        let committed = wallet::committed_pubkeys(&repo_root);
        wallet::refuse_committed(&wallet.pubkey(), "--wallet", &committed)?;
        if let Some(leader) = &leader {
            wallet::refuse_committed(&leader.pubkey(), "--leader", &committed)?;
        }
    }

    let rpc_url = args.cluster.rpc_url()?;
    if args.cluster.is_mainnet() {
        args.cluster.verify_genesis(&chain::rpc(&rpc_url))?;
        let leader_label = leader.as_ref().map(|k| k.pubkey().to_string());
        cluster::confirm_mainnet_entry(
            &rpc_url,
            &wallet.pubkey().to_string(),
            leader_label.as_deref(),
        )?;
    }

    let ctx = action::JobContext {
        rpc_url,
        cluster: args.cluster,
        repo_root,
        wallet_path,
        wallet,
        leader,
        // Mainnet never starts the managed container — it indexes the localnet.
        // The distinction is load-bearing (it stops `App`'s `Drop` tearing down
        // a container this session never brought up), so it lives in a tested
        // function rather than inline here.
        explorer_state: Arc::new(AtomicU8::new(explorer::initial_state(args.cluster))),
        explorer_lock: Arc::new(Mutex::new(())),
    };
    app::App::new(ctx)?.run(args.bootstrap)
}

fn print_help() {
    println!(
        "dropset-tui — control-plane TUI for the Dropset eCLOB\n\n\
         USAGE:\n    \
         dropset-tui [--cluster <localnet|mainnet>] [--wallet <path>] [--leader <path>] [--bootstrap]\n\n\
         OPTIONS:\n        \
         --cluster <name>    localnet (default) or mainnet\n    \
         -w, --wallet <path>     admin keypair (default: keys/BBBB.json)\n        \
         --leader <path>     mainnet only: vault leader keypair for create-vault / deposit\n        \
         --bootstrap         localnet only: run \"Bootstrap all\" once at launch\n    \
         -h, --help              show this help\n\n\
         MAINNET:\n    \
         Set {} to the JSON-RPC endpoint — there is deliberately no default.\n    \
         Entry verifies the genesis hash and requires a typed confirmation.",
        cluster::MAINNET_RPC_ENV
    );
}

/// Consume the value following an option, rejecting a missing value or a
/// flag-looking token (so `--cluster --bootstrap` errors instead of silently
/// taking `--bootstrap` as the cluster name).
fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    match it.next() {
        Some(v) if !v.starts_with('-') => Ok(v),
        Some(v) => bail!("{flag} needs a value, got flag-like `{v}`"),
        None => bail!("{flag} needs a value"),
    }
}

/// Parsed command line.
///
/// Hand-rolled to match the sibling `dropset-teardown` binary rather than to
/// save a dependency — and it replaced a looser parser that returned the first
/// non-dash token as the wallet path. That shape cannot survive an option that
/// takes a value: `--cluster mainnet` would have handed `mainnet` back as a
/// keypair path and then failed to load it, reporting a missing file rather
/// than a parse error.
struct Args {
    cluster: Cluster,
    wallet: Option<String>,
    leader: Option<String>,
    bootstrap: bool,
    help: bool,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut a = Args {
            cluster: Cluster::Localnet,
            wallet: None,
            leader: None,
            bootstrap: false,
            help: false,
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--cluster" => a.cluster = Cluster::parse(&value(&mut it, "--cluster")?)?,
                "--wallet" | "-w" => a.wallet = Some(value(&mut it, "--wallet")?),
                "--leader" => a.leader = Some(value(&mut it, "--leader")?),
                "--bootstrap" => a.bootstrap = true,
                "--help" | "-h" => a.help = true,
                // A bare path stays supported as the wallet, which is how the
                // existing localnet invocations pass it.
                s if !s.starts_with('-') && a.wallet.is_none() => a.wallet = Some(s.to_string()),
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
    fn leader_takes_a_value_and_defaults_to_none() {
        assert!(parse(&[]).unwrap().leader.is_none());
        let a = parse(&[
            "--cluster",
            "mainnet",
            "--leader",
            "/l.json",
            "-w",
            "/w.json",
        ])
        .unwrap();
        assert_eq!(a.leader.as_deref(), Some("/l.json"));
        assert_eq!(a.wallet.as_deref(), Some("/w.json"));
        assert!(parse(&["--leader"]).is_err());
        assert!(parse(&["--leader", "--bootstrap"]).is_err());
    }

    #[test]
    fn defaults_to_localnet_with_no_wallet_override() {
        let a = parse(&[]).unwrap();
        assert_eq!(a.cluster, Cluster::Localnet);
        assert!(a.wallet.is_none());
        assert!(!a.bootstrap);
    }

    #[test]
    fn cluster_flag_is_not_mistaken_for_a_wallet_path() {
        // The regression the old parser had: `mainnet` is the cluster's value,
        // never a positional keypair path.
        let a = parse(&["--cluster", "mainnet"]).unwrap();
        assert_eq!(a.cluster, Cluster::Mainnet);
        assert!(a.wallet.is_none());
    }

    #[test]
    fn bare_path_is_still_the_wallet() {
        let a = parse(&["/k.json"]).unwrap();
        assert_eq!(a.wallet.as_deref(), Some("/k.json"));
        assert_eq!(a.cluster, Cluster::Localnet);
    }

    #[test]
    fn flags_combine() {
        let a = parse(&["--cluster", "mainnet-beta", "-w", "/k.json"]).unwrap();
        assert_eq!(a.cluster, Cluster::Mainnet);
        assert_eq!(a.wallet.as_deref(), Some("/k.json"));
        let b = parse(&["--bootstrap"]).unwrap();
        assert!(b.bootstrap);
    }

    #[test]
    fn rejects_unknown_missing_and_flag_value() {
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--cluster"]).is_err()); // missing value
        assert!(parse(&["--cluster", "--bootstrap"]).is_err()); // flag swallowed
        assert!(parse(&["--cluster", "devnet"]).is_err()); // unknown cluster
        assert!(parse(&["--wallet"]).is_err());
    }
}
