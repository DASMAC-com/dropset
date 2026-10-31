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
//! real-funds banner. Before any of those, mainnet refuses an agent's shell
//! outright ([`dropset_tui::agent_shell`]): the operator runs it from their
//! own terminal.
//!
//! On mainnet the menu offers the ceremony — publish (or upgrade), init,
//! create-market, create-vault, deposit, close stranded buffers — and nothing
//! else (see [`action::Action::available_on`]). Each step shows its plan
//! under its own typed `yes`, re-checks the chain and refuses when its work
//! is already done, and references the real roster mints rather than minting
//! any. The create-vault step takes one more argument, typed at its prompt
//! rather than passed as a flag: the **vault name**, required on mainnet,
//! validated as the program validates it, shown in the step's plan and
//! recorded in the ceremony log (see [`dropset_tui::naming`]). Every key —
//! admin `--wallet`, `--leader`, `--quote`, `--program-key` — is a Solana
//! address, normally read from the TUI's keys file ([`dropset_tui::keys`]),
//! signed for by the SSH agent serving it, which prompts once per signature;
//! a launch refuses an address no agent serves. Every write is appended to
//! the ceremony log. The vault's quote authority is its own key: it pays for
//! and signs the ladder and re-centre alone.
//!
//! ```text
//! dropset-tui [--cluster <localnet|mainnet>] [--keys-file <path>] [--wallet <key>]
//!             [--leader <key>] [--quote <key>] [--program-key <key>]
//!             [--agent-sock <path>] [--ceremony-log <path>] [--bootstrap]
//!             [--rehearsal-loopback]
//! ```

use anyhow::{anyhow, bail, Result};
use dropset_tui::ceremony::{self, CeremonyLog};
use dropset_tui::cluster::{self, Cluster};
use dropset_tui::keys::{self, KeyAddresses, RoleKey};
use dropset_tui::{action, agent_shell, app, chain, explorer, fair, publish, wallet};
use solana_pubkey::Pubkey;
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
    // Mainnet is the operator's alone, from their own terminal — refused in
    // an agent's shell before any key reference is read.
    if args.cluster.is_mainnet() {
        agent_shell::refuse_agent_shell()?;
    }

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or_else(|| anyhow!("locate repo root from crate dir"))?
        .to_path_buf();
    // Mainnet keys come from the TUI's keys file, each line overridden by its
    // flag. Localnet signs with the committed roster, so a keys file there
    // would be silently ignored — refused instead, like `--leader`.
    let addresses = match &args.keys_file {
        None => KeyAddresses::default(),
        Some(_) if !args.cluster.is_mainnet() => {
            bail!("--keys-file is mainnet-only — localnet signs with the committed keys/ roster")
        }
        Some(path) => KeyAddresses::load(Path::new(path))?,
    };
    let wallet_arg = args.wallet.clone().or(addresses.wallet);
    let leader_arg = args.leader.clone().or(addresses.leader);
    let quote_arg = args.quote.clone().or(addresses.quote);
    let program_arg = args.program_key.clone().or(addresses.program_key);

    // The leader and program key are mainnet-only, like `--quote`: localnet
    // leads with the committed role keys and deploys the committed program
    // keypair through `make program`.
    if !args.cluster.is_mainnet() {
        if leader_arg.is_some() {
            bail!(
                "--leader is mainnet-only — the localnet roster leads with its committed role key"
            );
        }
        if program_arg.is_some() {
            bail!("--program-key is mainnet-only — localnet deploys the committed program keypair");
        }
        if args.agent_sock.is_some() {
            bail!("--agent-sock is mainnet-only — localnet signs with the committed keys/ roster");
        }
        if args.rehearsal_loopback {
            bail!("--rehearsal-loopback is mainnet-only — localnet has no mainnet check to swap");
        }
    }
    let agent_sock = args.agent_sock.as_deref().map(Path::new);

    // Refuse rather than ignore. Silently dropping the flag would leave the
    // operator believing the turnkey path ran, and "nothing happened" is
    // indistinguishable from "it worked" until the book is inspected.
    if args.bootstrap && args.cluster.is_mainnet() {
        bail!(
            "--bootstrap is a localnet-only turnkey path (it deploys the program \
             and mints mock tokens) and has no mainnet meaning"
        );
    }

    // Localnet reads the committed key files. Mainnet names each role by
    // address and finds the SSH agent serving it now, so a key the agent does
    // not serve refuses the launch before the banner rather than a step
    // mid-session. Mainnet has no default admin (the default is the committed
    // localnet one).
    let mut sockets: Vec<PathBuf> = Vec::new();
    let mut resolve = |arg: &str, role: &str| -> Result<RoleKey> {
        let (key, socket) = wallet::resolve_key(arg, role, agent_sock)?;
        if let Some(socket) = socket.filter(|s| !sockets.contains(s)) {
            sockets.push(socket);
        }
        Ok(key)
    };
    let (wallet, wallet_path): (RoleKey, String) = if args.cluster.is_mainnet() {
        let arg = wallet_arg.as_deref().ok_or_else(|| {
            anyhow!(
                "mainnet needs the wallet key: {} in the keys file or --wallet — a Solana \
                 address the SSH agent serves, or `{}` for a watch-only session",
                keys::WALLET_VAR,
                wallet::EPHEMERAL_WALLET
            )
        })?;
        (resolve(arg, "--wallet")?, arg.to_string())
    } else {
        let (kp, path) = wallet::load(wallet_arg.as_deref(), &repo_root)?;
        (Arc::new(kp), path)
    };
    let watch_only = wallet_arg.as_deref() == Some(wallet::EPHEMERAL_WALLET);
    let quote = quote_address(args.cluster, quote_arg, watch_only)?
        .map(|arg| resolve(&arg, "--quote"))
        .transpose()?;
    let leader = leader_arg
        .map(|arg| resolve(&arg, "--leader"))
        .transpose()?;
    let program_key = program_arg
        .map(|arg| resolve(&arg, "--program-key"))
        .transpose()?;

    if args.cluster.is_mainnet() {
        let roles = [
            ("--wallet", Some(wallet.pubkey())),
            ("--leader", leader.as_ref().map(|k| k.pubkey())),
            ("--quote", quote.as_ref().map(|k| k.pubkey())),
            ("--program-key", program_key.as_ref().map(|k| k.pubkey())),
        ];
        // The program rejects one key as both admin and leader, and a shared
        // program key would fail the deploy only after every buffer write.
        refuse_shared_roles(&roles)?;
        // A committed keypair's secret is public, so on mainnet no role may be one.
        let committed = wallet::committed_pubkeys(&repo_root);
        for (role, key) in roles {
            if let Some(key) = key {
                wallet::refuse_committed(&key, role, &committed)?;
            }
        }
    }

    // The ceremony record: always kept on mainnet, opt-in on localnet. Opened
    // before the banner so a log that cannot be written refuses the session
    // rather than surfacing after a write has landed unrecorded.
    let mainnet_log = args
        .cluster
        .is_mainnet()
        .then(|| ceremony::default_path(args.cluster))
        .flatten();
    let log_path = ceremony_log_path(
        args.cluster,
        args.rehearsal_loopback,
        args.ceremony_log.as_deref(),
        mainnet_log,
    )?;
    let ceremony_log = log_path
        .map(|path| CeremonyLog::open(&path, &repo_root, args.cluster).map(Arc::new))
        .transpose()?;

    let rpc_url = args.cluster.rpc_url()?;
    if args.cluster.is_mainnet() {
        args.cluster
            .verify_genesis(&chain::rpc(&rpc_url), args.rehearsal_loopback)?;
        let leader_label = leader.as_ref().map(|k| k.pubkey().to_string());
        let quote_label = quote.as_ref().map(|k| k.pubkey().to_string());
        let program_label = program_key.as_ref().map(|k| k.pubkey().to_string());
        let agent_label = (!sockets.is_empty()).then(|| {
            sockets
                .iter()
                .map(|s| s.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        });
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
            agent_label.as_deref(),
            &log_label,
        )?;
    }

    let ctx = action::JobContext {
        rpc_url,
        cluster: args.cluster,
        rehearsal_loopback: args.rehearsal_loopback,
        repo_root,
        wallet_path,
        wallet,
        leader,
        quote,
        program_key,
        stranded: publish::Stranded::default(),
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
         [--agent-sock <path>] [--ceremony-log <path>] [--bootstrap]\n                \
         [--rehearsal-loopback]\n\n\
         OPTIONS:\n        \
         --cluster <name>        localnet (default) or mainnet\n        \
         --keys-file <path>      mainnet only: the key-addresses file (tui/private-keys.env)\n    \
         -w, --wallet <key>          admin key (default on localnet: keys/BBBB.json)\n        \
         --leader <key>          mainnet only: vault leader (custody)\n        \
         --quote <key>           mainnet only: vault quote authority (ladder, re-centre)\n        \
         --program-key <key>     mainnet only: program key, for a first publish\n        \
         --agent-sock <path>     mainnet only: the SSH agent socket, skipping discovery\n        \
         --ceremony-log <path>   append-only record of every write (mainnet default:\n                                \
         ${} or ~/.dropset/ceremony-mainnet-beta.log)\n        \
         --bootstrap             localnet only: run \"Bootstrap all\" once at launch\n        \
         --rehearsal-loopback    mainnet only: dress rehearsal against a loopback\n                                \
         validator (needs --ceremony-log)\n    \
         -h, --help                  show this help\n\n\
         KEYS:\n    \
         Localnet keys are keypair files. Mainnet keys are Solana addresses, each\n    \
         signed for by the SSH agent that serves it: SSH_AUTH_SOCK, then\n    \
         ~/.1password/agent.sock, then 1Password's own socket, or --agent-sock.\n    \
         Launch refuses an address no agent serves, and the agent prompts once\n    \
         per signature. `--wallet {}` is a fresh unfunded key. The keys file\n    \
         holds one address per line ({}, {}, {}, {}); a flag\n    \
         overrides its line. The quote key is required unless watch-only, and\n    \
         pays for the ladder and re-centre.\n\n\
         MAINNET:\n    \
         Set {} to the JSON-RPC endpoint — there is deliberately no default.\n    \
         Entry verifies the genesis hash and requires a typed confirmation, and\n    \
         every ceremony step shows its plan under its own typed confirmation.\n    \
         --rehearsal-loopback instead admits only a 127.0.0.1 or localhost\n    \
         endpoint whose genesis is NOT mainnet-beta, and needs its own\n    \
         --ceremony-log; every other mainnet behavior (agent signer, keys,\n    \
         confirmations) is unchanged.\n\n\
         FAIR PRICE:\n    \
         Optional. Set {} to a Postgres URL logging in as {}\n    \
         (localnet: postgres://dropset_ro:dropset_ro@127.0.0.1:5432/dropset) and a\n    \
         blank passive center is the estimator's fair; a paused or stale fair\n    \
         refuses the blank. Unset, it is the vault's live reference or the book mid.\n    \
         A remote host needs sslmode=verify-full (sslrootcert=<CA file> for a\n    \
         private CA).",
        ceremony::CEREMONY_LOG_ENV,
        wallet::EPHEMERAL_WALLET,
        keys::WALLET_VAR,
        keys::LEADER_VAR,
        keys::QUOTE_VAR,
        keys::PROGRAM_KEY_VAR,
        cluster::MAINNET_RPC_ENV,
        fair::DATABASE_URL_ENV,
        fair::READER_ROLE
    );
}

/// The quote-authority address a launch resolves — the hot-path signer and
/// payer.
///
/// Mainnet-only, and refused on localnet for the same reason as
/// `--bootstrap`: the localnet roster's committed leader is also its quote
/// authority, so a supplied one would be silently ignored. Required on
/// mainnet unless the session is watch-only, naming the missing key, so no
/// path can fall back to the leader for it.
fn quote_address(
    cluster: Cluster,
    address: Option<String>,
    watch_only: bool,
) -> Result<Option<String>> {
    match address {
        Some(_) if !cluster.is_mainnet() => bail!(
            "--quote is mainnet-only — the localnet roster's committed leader is its quote authority"
        ),
        None if cluster.is_mainnet() && !watch_only => bail!(
            "mainnet needs the quote key: {} in the keys file or --quote <address> — it signs \
             the ladder and re-centre, and create-vault sets it as each vault's quote authority",
            keys::QUOTE_VAR
        ),
        address => Ok(address),
    }
}

/// The ceremony log a launch opens: `given` (`--ceremony-log`) wins, else
/// mainnet's default `mainnet_log`, else none.
///
/// A rehearsal must name its own log, and never the mainnet default: that
/// file is the real ceremony record, and its lines carry no rehearsal mark,
/// so a localnet signature appended there could not be told from a real one.
fn ceremony_log_path(
    cluster: Cluster,
    rehearsal_loopback: bool,
    given: Option<&str>,
    mainnet_log: Option<PathBuf>,
) -> Result<Option<PathBuf>> {
    match given {
        Some(path) if rehearsal_loopback => {
            let path = PathBuf::from(path);
            if mainnet_log.is_some_and(|real| resolved(&real) == resolved(&path)) {
                bail!("--ceremony-log is the mainnet ceremony record — a rehearsal names its own");
            }
            Ok(Some(path))
        }
        Some(path) => Ok(Some(PathBuf::from(path))),
        None if rehearsal_loopback => {
            bail!("--rehearsal-loopback needs --ceremony-log, kept apart from the mainnet record")
        }
        None if cluster.is_mainnet() => mainnet_log.map(Some).ok_or_else(|| {
            anyhow!(
                "no ceremony log path — set {} or pass --ceremony-log",
                ceremony::CEREMONY_LOG_ENV
            )
        }),
        None => Ok(None),
    }
}

/// `path` with its directory resolved, so two spellings of one file compare
/// equal whether or not the file exists yet.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(full) = std::fs::canonicalize(path) {
        return full;
    }
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    match (
        std::fs::canonicalize(dir.unwrap_or(Path::new("."))),
        path.file_name(),
    ) {
        (Ok(dir), Some(name)) => dir.join(name),
        _ => path.to_path_buf(),
    }
}

/// Refuse a launch in which two `roles` name the same key.
fn refuse_shared_roles(roles: &[(&str, Option<Pubkey>)]) -> Result<()> {
    for (i, (role, key)) in roles.iter().enumerate() {
        for (other, other_key) in &roles[i + 1..] {
            if key.is_some() && key == other_key {
                bail!("{role} must differ from {other}");
            }
        }
    }
    Ok(())
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
    agent_sock: Option<String>,
    ceremony_log: Option<String>,
    bootstrap: bool,
    rehearsal_loopback: bool,
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
            agent_sock: None,
            ceremony_log: None,
            bootstrap: false,
            rehearsal_loopback: false,
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
                "--agent-sock" => a.agent_sock = Some(value(&mut it, "--agent-sock")?),
                "--ceremony-log" => a.ceremony_log = Some(value(&mut it, "--ceremony-log")?),
                "--bootstrap" => a.bootstrap = true,
                "--rehearsal-loopback" => a.rehearsal_loopback = true,
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
            "tui/private-keys.env",
            "--quote",
            "<quote-address>",
        ])
        .unwrap();
        assert_eq!(a.keys_file.as_deref(), Some("tui/private-keys.env"));
        assert_eq!(a.quote.as_deref(), Some("<quote-address>"));
        assert!(parse(&["--quote"]).is_err());
        assert!(parse(&["--keys-file", "--bootstrap"]).is_err());
    }

    #[test]
    fn a_mainnet_launch_without_a_quote_key_is_refused_naming_it() {
        let err = quote_address(Cluster::Mainnet, None, false)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains(keys::QUOTE_VAR) && err.contains("--quote"),
            "{err}"
        );
        // A watch-only session signs nothing, so it needs none.
        assert_eq!(quote_address(Cluster::Mainnet, None, true).unwrap(), None);
        let given = Some("<quote-address>".to_string());
        assert_eq!(
            quote_address(Cluster::Mainnet, given.clone(), false).unwrap(),
            given
        );
        // Localnet's committed leader is its quote authority: a supplied key
        // would be ignored, so it is refused, and none is required.
        assert!(quote_address(Cluster::Localnet, given, false).is_err());
        assert_eq!(quote_address(Cluster::Localnet, None, false).unwrap(), None);
    }

    #[test]
    fn ceremony_flags_take_values() {
        let a = parse(&[
            "--cluster",
            "mainnet",
            "--program-key",
            "<program-address>",
            "--ceremony-log",
            "/tmp/c.log",
        ])
        .unwrap();
        assert_eq!(a.program_key.as_deref(), Some("<program-address>"));
        assert_eq!(a.ceremony_log.as_deref(), Some("/tmp/c.log"));
        assert!(parse(&["--program-key"]).is_err());
        let b = parse(&["--cluster", "mainnet", "--agent-sock", "/tmp/a.sock"]).unwrap();
        assert_eq!(b.agent_sock.as_deref(), Some("/tmp/a.sock"));
        assert!(parse(&["--agent-sock", "--bootstrap"]).is_err());
        assert!(parse(&["--ceremony-log", "--bootstrap"]).is_err());
    }

    #[test]
    fn rehearsal_loopback_is_an_opt_in_switch() {
        assert!(!parse(&["--cluster", "mainnet"]).unwrap().rehearsal_loopback);
        let a = parse(&["--cluster", "mainnet", "--rehearsal-loopback"]).unwrap();
        assert!(a.rehearsal_loopback);
        // A switch, not an option: it takes no value, so the next flag parses.
        let b = parse(&["--rehearsal-loopback", "--cluster", "mainnet"]).unwrap();
        assert!(b.rehearsal_loopback && b.cluster.is_mainnet());
    }

    #[test]
    fn a_rehearsal_names_its_own_ceremony_log() {
        let dir = std::env::temp_dir().join(format!("dropset-rehearsal-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("ceremony-mainnet-beta.log");
        let main = Cluster::Mainnet;
        // A plain mainnet launch falls back to the real record.
        let plain = ceremony_log_path(main, false, None, Some(real.clone())).unwrap();
        assert_eq!(plain, Some(real.clone()));
        // A rehearsal with no log, or with the real record by another spelling, refuses.
        assert!(ceremony_log_path(main, true, None, Some(real.clone())).is_err());
        let alias = dir.join(".").join("ceremony-mainnet-beta.log");
        let alias = alias.to_str().unwrap();
        assert!(ceremony_log_path(main, true, Some(alias), Some(real.clone())).is_err());
        // Its own file is accepted.
        let own = dir.join("rehearsal.log");
        let own = own.to_str().unwrap();
        let got = ceremony_log_path(main, true, Some(own), Some(real)).unwrap();
        assert_eq!(got, Some(PathBuf::from(own)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn two_roles_sharing_a_key_are_refused_by_name() {
        let (a, b) = (Pubkey::new_unique(), Pubkey::new_unique());
        let err = refuse_shared_roles(&[
            ("--wallet", Some(a)),
            ("--leader", None),
            ("--quote", Some(a)),
        ])
        .unwrap_err()
        .to_string();
        assert_eq!(err, "--wallet must differ from --quote");
        // Absent roles never collide with each other.
        refuse_shared_roles(&[
            ("--wallet", Some(a)),
            ("--leader", None),
            ("--quote", Some(b)),
            ("--program-key", None),
        ])
        .unwrap();
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
