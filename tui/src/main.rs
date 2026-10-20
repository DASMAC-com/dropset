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
//! is built, and the localnet bootstrap and wipe are unavailable. Entry is
//! guarded three ways, each catching something the others cannot — the
//! endpoint must be named explicitly by environment variable, its genesis hash
//! must actually be mainnet-beta's, and the operator must type `yes` to a
//! real-funds banner.
//!
//! On mainnet the menu offers the ceremony — publish (or upgrade), init,
//! create-market, create-vault, deposit, close stranded buffers — and nothing
//! else (see [`action::Action::available_on`]). Each step shows its plan
//! under its own typed `yes`, re-checks the chain and refuses when its work
//! is already done, and references the real roster mints rather than minting
//! any. Every key — admin `--wallet`, `--leader`, `--quote`, `--program-key`
//! — is a 1Password reference resolved in memory, normally read from the
//! TUI's keys file ([`dropset_tui::keys`]), and every write is appended to the
//! ceremony log. The vault's quote authority is its own key: the ladder and
//! re-centre sign with it, and the leader — the custody key — resolves only
//! when a custody step first needs it.
//!
//! ```text
//! dropset-tui [--cluster <localnet|mainnet>] [--keys-file <path>] [--wallet <key>]
//!             [--leader <key>] [--quote <key>] [--program-key <key>]
//!             [--ceremony-log <path>] [--bootstrap]
//! ```

use anyhow::{anyhow, bail, Result};
use dropset_tui::ceremony::{self, CeremonyLog};
use dropset_tui::cluster::{self, Cluster};
use dropset_tui::keys::{self, KeyRefs, LazyKey};
use dropset_tui::{action, app, chain, explorer, wallet};
use solana_signer::Signer;
use std::path::{Path, PathBuf};
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
    // Mainnet keys come from the TUI's keys file, each line overridden by its
    // flag. Localnet signs with the committed roster, so a keys file there
    // would be silently ignored — refused instead, like `--leader`.
    let refs = match &args.keys_file {
        None => KeyRefs::default(),
        Some(_) if !args.cluster.is_mainnet() => {
            bail!("--keys-file is mainnet-only — localnet signs with the committed keys/ roster")
        }
        Some(path) => KeyRefs::load(Path::new(path))?,
    };
    let wallet_ref = args.wallet.clone().or(refs.wallet);
    let leader_ref = args.leader.clone().or(refs.leader);
    let quote_ref = args.quote.clone().or(refs.quote);

    // The operator's vault leader — the custody key. Mainnet-only, refused on
    // localnet like `--quote`. It resolves only when a custody step first
    // needs it (below), but its shape is checked now, before any `op read`
    // prompt, so a key file is refused at launch rather than mid-session.
    match &leader_ref {
        Some(_) if !args.cluster.is_mainnet() => bail!(
            "--leader is mainnet-only — the localnet roster leads with its committed role key"
        ),
        Some(arg) if !arg.starts_with(wallet::OP_REFERENCE_PREFIX) => bail!(
            "--leader on mainnet must be a 1Password reference ({}vault/item/field) — key \
             files are refused on a real cluster",
            wallet::OP_REFERENCE_PREFIX
        ),
        _ => {}
    }

    // Localnet reads the committed key files; mainnet resolves 1Password
    // references in memory and has no default admin (the default is the
    // committed localnet one).
    let (wallet, wallet_path) = if args.cluster.is_mainnet() {
        let arg = wallet_ref.as_deref().ok_or_else(|| {
            anyhow!(
                "mainnet needs the wallet key: {} in the keys file or --wallet — a 1Password \
                 reference (op://…), or `{}` for a watch-only session",
                keys::WALLET_VAR,
                wallet::EPHEMERAL_WALLET
            )
        })?;
        let kp = wallet::resolve_key(arg, "--wallet", args.cluster)?;
        (kp, arg.to_string())
    } else {
        wallet::load(wallet_ref.as_deref(), &repo_root)?
    };
    let watch_only = wallet_ref.as_deref() == Some(wallet::EPHEMERAL_WALLET);

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
    let quote = match quote_reference(args.cluster, quote_ref, watch_only)? {
        None => None,
        Some(arg) => {
            let kp = wallet::resolve_key(&arg, "--quote", args.cluster)?;
            if kp.pubkey() == wallet.pubkey() {
                bail!("--quote must differ from --wallet");
            }
            Some(kp)
        }
    };

    // The program key signs only a first publish. Mainnet-only: localnet
    // deploys with the committed program keypair through `make program`.
    let program_key = match &args.program_key {
        None => None,
        Some(_) if !args.cluster.is_mainnet() => {
            bail!("--program-key is mainnet-only — localnet deploys the committed program keypair")
        }
        Some(arg) => {
            let kp = wallet::resolve_key(arg, "--program-key", args.cluster)?;
            // Refused now rather than at deploy, where creating the program
            // account would fail only after every buffer write was paid for.
            // The leader is checked against it when the leader resolves.
            let other = quote.as_ref().map(Signer::pubkey);
            if kp.pubkey() == wallet.pubkey() || Some(kp.pubkey()) == other {
                bail!("--program-key must differ from --wallet and --quote");
            }
            Some(kp)
        }
    };

    // A committed keypair's secret is public, so on mainnet no role may be one.
    let committed = wallet::committed_pubkeys(&repo_root);
    if args.cluster.is_mainnet() {
        wallet::refuse_committed(&wallet.pubkey(), "--wallet", &committed)?;
        if let Some(quote) = &quote {
            wallet::refuse_committed(&quote.pubkey(), "--quote", &committed)?;
        }
        if let Some(program) = &program_key {
            wallet::refuse_committed(&program.pubkey(), "--program-key", &committed)?;
        }
    }

    // Only the leader's reference is taken here: it resolves when a custody
    // step first needs it, running the checks the other roles ran above, so a
    // session that only re-centres never loads it.
    let leader = leader_ref.map(|arg| {
        // The program rejects one key as both admin and leader.
        let mut distinct_from = vec![(wallet.pubkey(), "--wallet")];
        distinct_from.extend(quote.as_ref().map(|k| (k.pubkey(), "--quote")));
        distinct_from.extend(program_key.as_ref().map(|k| (k.pubkey(), "--program-key")));
        Arc::new(LazyKey::new(
            arg,
            "--leader",
            args.cluster,
            distinct_from,
            committed,
        ))
    });

    // The ceremony record: always kept on mainnet, opt-in on localnet. Opened
    // before the banner so a log that cannot be written refuses the session
    // rather than surfacing after a write has landed unrecorded.
    let log_path = match &args.ceremony_log {
        Some(path) => Some(PathBuf::from(path)),
        None if args.cluster.is_mainnet() => {
            Some(ceremony::default_path(args.cluster).ok_or_else(|| {
                anyhow!(
                    "no ceremony log path — set {} or pass --ceremony-log",
                    ceremony::CEREMONY_LOG_ENV
                )
            })?)
        }
        None => None,
    };
    let ceremony_log = log_path
        .map(|path| CeremonyLog::open(&path, &repo_root, args.cluster).map(Arc::new))
        .transpose()?;

    let rpc_url = args.cluster.rpc_url()?;
    if args.cluster.is_mainnet() {
        args.cluster.verify_genesis(&chain::rpc(&rpc_url))?;
        // The reference, not a pubkey: the leader is not resolved yet, and the
        // reference is what names the custody item to check before `yes`.
        let leader_label = leader
            .as_ref()
            .map(|k| format!("{} (resolved on the first custody step)", k.reference()));
        let quote_label = quote.as_ref().map(|k| k.pubkey().to_string());
        let program_label = program_key.as_ref().map(|k| k.pubkey().to_string());
        let log_label = ceremony_log
            .as_ref()
            .map(|l| l.path().display().to_string())
            .unwrap_or_default();
        cluster::confirm_mainnet_entry(
            &rpc_url,
            &wallet.pubkey().to_string(),
            leader_label.as_deref(),
            quote_label.as_deref(),
            program_label.as_deref(),
            &log_label,
        )?;
    }

    let ctx = action::JobContext {
        rpc_url,
        cluster: args.cluster,
        repo_root,
        wallet_path,
        wallet,
        leader,
        quote,
        program_key,
        ceremony_log,
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
         dropset-tui [--cluster <localnet|mainnet>] [--keys-file <path>] [--wallet <key>]\n                \
         [--leader <key>] [--quote <key>] [--program-key <key>]\n                \
         [--ceremony-log <path>] [--bootstrap]\n\n\
         OPTIONS:\n        \
         --cluster <name>        localnet (default) or mainnet\n        \
         --keys-file <path>      mainnet only: the key-references file (tui/keys.env)\n    \
         -w, --wallet <key>          admin keypair (default on localnet: keys/BBBB.json)\n        \
         --leader <key>          mainnet only: vault leader (custody), resolved on first use\n        \
         --quote <key>           mainnet only: vault quote authority (ladder, re-centre)\n        \
         --program-key <key>     mainnet only: program keypair, for a first publish\n        \
         --ceremony-log <path>   append-only record of every write (mainnet default:\n                                \
         ${} or ~/.dropset/ceremony-mainnet-beta.log)\n        \
         --bootstrap             localnet only: run \"Bootstrap all\" once at launch\n    \
         -h, --help                  show this help\n\n\
         KEYS:\n    \
         Localnet keys are keypair files. Mainnet keys are 1Password references\n    \
         (op://vault/item/field, read with `op read`; set {} to pick the\n    \
         account) — files are refused. `--wallet {}` is a fresh unfunded key.\n    \
         The keys file holds one reference per line ({}, {}, {});\n    \
         a flag overrides its line. The quote key is required unless watch-only.\n\n\
         MAINNET:\n    \
         Set {} to the JSON-RPC endpoint — there is deliberately no default.\n    \
         Entry verifies the genesis hash and requires a typed confirmation, and\n    \
         every ceremony step shows its plan under its own typed confirmation.",
        ceremony::CEREMONY_LOG_ENV,
        wallet::OP_ACCOUNT_ENV,
        wallet::EPHEMERAL_WALLET,
        keys::WALLET_VAR,
        keys::LEADER_VAR,
        keys::QUOTE_VAR,
        cluster::MAINNET_RPC_ENV
    );
}

/// The quote-authority reference a launch resolves — the hot-path signer.
///
/// Mainnet-only, and refused on localnet for the same reason as
/// `--bootstrap`: the localnet roster's committed leader is also its quote
/// authority, so a supplied one would be silently ignored. Required on
/// mainnet unless the session is watch-only, naming the missing key, so no
/// path can fall back to the leader for it.
fn quote_reference(
    cluster: Cluster,
    reference: Option<String>,
    watch_only: bool,
) -> Result<Option<String>> {
    match reference {
        Some(_) if !cluster.is_mainnet() => bail!(
            "--quote is mainnet-only — the localnet roster's committed leader is its quote authority"
        ),
        None if cluster.is_mainnet() && !watch_only => bail!(
            "mainnet needs the quote key: {} in the keys file or --quote op://… — it signs \
             the ladder and re-centre, and create-vault sets it as each vault's quote authority",
            keys::QUOTE_VAR
        ),
        reference => Ok(reference),
    }
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
    quote: Option<String>,
    keys_file: Option<String>,
    program_key: Option<String>,
    ceremony_log: Option<String>,
    bootstrap: bool,
    help: bool,
}

impl Args {
    fn parse(mut it: impl Iterator<Item = String>) -> Result<Self> {
        let mut a = Args {
            cluster: Cluster::Localnet,
            wallet: None,
            leader: None,
            quote: None,
            keys_file: None,
            program_key: None,
            ceremony_log: None,
            bootstrap: false,
            help: false,
        };
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--cluster" => a.cluster = Cluster::parse(&value(&mut it, "--cluster")?)?,
                "--wallet" | "-w" => a.wallet = Some(value(&mut it, "--wallet")?),
                "--leader" => a.leader = Some(value(&mut it, "--leader")?),
                "--quote" => a.quote = Some(value(&mut it, "--quote")?),
                "--keys-file" => a.keys_file = Some(value(&mut it, "--keys-file")?),
                "--program-key" => a.program_key = Some(value(&mut it, "--program-key")?),
                "--ceremony-log" => a.ceremony_log = Some(value(&mut it, "--ceremony-log")?),
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
    fn quote_and_keys_file_take_values() {
        let a = parse(&[
            "--cluster",
            "mainnet",
            "--keys-file",
            "tui/keys.env",
            "--quote",
            "op://v/quote/key",
        ])
        .unwrap();
        assert_eq!(a.keys_file.as_deref(), Some("tui/keys.env"));
        assert_eq!(a.quote.as_deref(), Some("op://v/quote/key"));
        assert!(parse(&["--quote"]).is_err());
        assert!(parse(&["--keys-file", "--bootstrap"]).is_err());
    }

    #[test]
    fn a_mainnet_launch_without_a_quote_key_is_refused_naming_it() {
        let err = quote_reference(Cluster::Mainnet, None, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(keys::QUOTE_VAR) && err.contains("--quote"),
            "{err}"
        );
        // A watch-only session signs nothing, so it needs none.
        assert_eq!(quote_reference(Cluster::Mainnet, None, true).unwrap(), None);
        let given = Some("op://v/quote/key".to_string());
        assert_eq!(
            quote_reference(Cluster::Mainnet, given.clone(), false).unwrap(),
            given
        );
        // Localnet's committed leader is its quote authority: a supplied key
        // would be ignored, so it is refused, and none is required.
        assert!(quote_reference(Cluster::Localnet, given, false).is_err());
        assert_eq!(
            quote_reference(Cluster::Localnet, None, false).unwrap(),
            None
        );
    }

    #[test]
    fn ceremony_flags_take_values() {
        let a = parse(&[
            "--cluster",
            "mainnet",
            "--program-key",
            "op://v/program/key",
            "--ceremony-log",
            "/tmp/c.log",
        ])
        .unwrap();
        assert_eq!(a.program_key.as_deref(), Some("op://v/program/key"));
        assert_eq!(a.ceremony_log.as_deref(), Some("/tmp/c.log"));
        assert!(parse(&["--program-key"]).is_err());
        assert!(parse(&["--ceremony-log", "--bootstrap"]).is_err());
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
