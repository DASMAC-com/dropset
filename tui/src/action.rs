//! The action menu: what each entry does, when it is enabled, and how it
//! dispatches to a background job.
//!
//! Availability has two independent gates. [`Action::enabled`] is a pure
//! function of the derived [`Phase`] — the panel is always truthful about what
//! is possible right now, and greys out the rest with a one-line reason.
//! [`Action::available_on`] is a pure function of the [`Cluster`], and gates
//! on what an action *means* rather than on what the chain currently holds:
//! spawning a validator, airdropping SOL, or throwing a ledger away has no
//! mainnet counterpart at all.
//!
//! The two are separate because they answer different questions and fail
//! differently. A phase gate is a "not yet" that resolves as the chain moves;
//! a cluster gate is a "not here" that no amount of waiting changes. Folding
//! them together would report the second as the first, which is how an
//! operator ends up waiting for a step that is never coming.
//!
//! Bootstrapping is a sequence of discrete gated steps (deploy → init →
//! create-market → create-vault → deposit) so each account and its rent can be
//! watched appearing one at a time; "Bootstrap all" chains the whole sequence —
//! deploying the program first when it isn't yet on-chain — for convenience.
//!
//! Both gates above read a poll that can be most of a second old, so neither
//! is what stops a double-create: a stale poll plus a keypress would pass
//! them. That job belongs to a **third** check, inside each ceremony function,
//! which reads the chain fresh at execution time and refuses — nothing sent —
//! when the thing it would create is already there (see `Outcome`). A failed
//! read refuses too, rather than reading as "absent".

use crate::accounts::{self, ChainState, Phase, VaultSeat};
use crate::chain;
use crate::cluster::Cluster;
use crate::deploy;
use crate::explorer;
use crate::job::{self, JobEvent, Logger};
use crate::market::{self, PairConfig};
use crate::teardown;
use anyhow::{Context, Result};
use dropset_sdk::clock::WallSpan;
use dropset_sdk::matching::SwapSide;
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{set_liquidity_profile_ix, set_reference_price_ix};
use dropset_util::decimals::atoms_ratio_to_human;
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// A menu action.
///
/// The first block is the bootstrap-lifecycle plus the utility actions; the
/// numbered `1..=8` entries in [`MENU`] are drawn from it (the swap is a
/// runtime control reached via `s`, not a numbered entry). The trailing block
/// are the eCLOB demo controls —
/// market-scoped keybinds, *not* menu entries — that fire the two quoting
/// instructions independently on the selected market to show the "reprice vs
/// reshape" distinction live: [`Action::RepegUp`] / [`Action::RepegDown`] move
/// the whole ladder (`set_reference_price`), while the reshape actions change
/// the ladder's shape at a fixed peg (`set_liquidity_profile`).
///
/// Both compete with a running maker bot, which re-quotes every tick and
/// overwrites a manual nudge within ~1s — stop the market's bot (`m` / `x`)
/// for a stable on-stage demo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Deploy,
    InitRegistry,
    CreateMarket,
    CreateVault,
    /// The one-shot seed deposit into each roster vault: shape the ladder,
    /// then `deposit_leader` the opening inventory.
    Deposit,
    OpenExplorer,
    BootstrapAll,
    ProbeSwap,
    Teardown,
    Wipe,
    /// The leader tops up its own seeded vault (one quote leg; the program
    /// derives the base). Opens the amount → typed-`yes` prompt rather than
    /// dispatching directly — see [`crate::leader`].
    LeaderDeposit,
    /// The leader draws a percentage of its own stake back out. Same prompt.
    LeaderWithdraw,
    /// Reserved, deliberately unimplemented: the leader-rotation instruction
    /// arrives with its program change, and a builder guessed at its account
    /// shape now would be rework by construction. Never enabled.
    RotateLeader,
    // eCLOB demo controls — keybinds, not menu entries.
    RepegUp,
    RepegDown,
    WidenSpread,
    TightenSpread,
    ThinFarSide,
    ResetLadder,
    /// Reset every market's ladder to the default shape in one job — the
    /// broadcast form of [`Action::ResetLadder`], for re-arming the whole demo
    /// after nudging several books.
    ResetAllLadders,
}

/// Whole book shift per reprice nudge — ±5 bps re-anchors the ladder without
/// touching its shape.
const REPEG_BPS: f64 = 5.0;
/// `Price::quote_for_base` scale for decoding a reference to its atoms-ratio
/// before the bump — matches the maker bot and SDK (`value × 10^9`).
const PRICE_SCALE: u64 = 1_000_000_000;
/// "Thin the far side" scales the ask ladder's per-rung depth to a fraction of
/// full, so the offer side visibly shrinks across every level while the bid
/// stays at the full ladder.
const THIN_DEPTH_SCALE: f64 = 0.3;
/// The demo presets quote until the next reshape (the maker bot re-arms
/// expiry itself).
///
/// An alias for the wall domain's one named unbounded constant rather
/// than a second bare `u32::MAX`: the slot domain has always had
/// `SlotSpan::UNBOUNDED`, and the two domains disagreeing on how to
/// spell the same idea is what let a raw `u32::MAX` sit here reading as
/// a magic number.
const NEVER_EXPIRES: WallSpan = WallSpan::UNBOUNDED;

/// The numbered setup menu in display order — the bootstrap lifecycle plus the
/// explorer / teardown / wipe utilities, then the leader stake commands.
/// Indices map to the `1..=9` number keys; the leader entries sit past them,
/// reached with `j`/`k` + Enter (or `+` / `-`), so appending them moved no
/// existing key. The swap is deliberately absent: it is a runtime control,
/// reached only via `s` (and listed in the "runtime" pane), so it appears in
/// exactly one place rather than doubling as a numbered step.
pub const MENU: [Action; 12] = [
    Action::Deploy,
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
    Action::Deposit,
    Action::OpenExplorer,
    Action::BootstrapAll,
    Action::Teardown,
    Action::Wipe,
    Action::LeaderDeposit,
    Action::LeaderWithdraw,
    Action::RotateLeader,
];

/// The mainnet menu — every entry that is meaningful against real funds.
///
/// The four ceremony steps and the explorer, then the leader stake commands.
/// Each ceremony step is one-shot and existence-checked at execution time,
/// references the real roster mints rather than minting any (see
/// [`market::MAINNET_PAIRS`]), and signs vault steps with the
/// operator-supplied leader; the leader commands send only after a typed
/// `yes` over the exact amounts. Nothing here airdrops, deploys or discards a
/// ledger.
///
/// Kept in sync with [`Action::available_on`] by a test rather than by care —
/// two hand-maintained lists of the same fact drift, and the direction it
/// would drift here is toward exposing a write.
pub const MAINNET_MENU: [Action; 8] = [
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
    Action::Deposit,
    Action::OpenExplorer,
    Action::LeaderDeposit,
    Action::LeaderWithdraw,
    Action::RotateLeader,
];

/// The menu for `cluster`, in display order.
pub fn menu_for(cluster: Cluster) -> &'static [Action] {
    match cluster {
        Cluster::Localnet => &MENU,
        Cluster::Mainnet => &MAINNET_MENU,
    }
}

/// The ordered bootstrap steps, used to pick the recommended next step.
const BOOTSTRAP: [Action; 5] = [
    Action::Deploy,
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
    Action::Deposit,
];

impl Action {
    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Action::Deploy => "Deploy program",
            Action::InitRegistry => "Init registry",
            Action::CreateMarket => "Create market",
            Action::CreateVault => "Create vault",
            Action::Deposit => "Seed deposit",
            Action::OpenExplorer => "Open explorer",
            Action::BootstrapAll => "Bootstrap all",
            Action::ProbeSwap => "Probe swap (CU)",
            Action::Teardown => "Teardown & reclaim",
            Action::Wipe => "Wipe localnet",
            Action::LeaderDeposit => "Leader deposit",
            Action::LeaderWithdraw => "Leader withdraw",
            Action::RotateLeader => "Rotate leader",
            Action::RepegUp => "Re-peg +5 bps",
            Action::RepegDown => "Re-peg -5 bps",
            Action::WidenSpread => "Widen spread",
            Action::TightenSpread => "Tighten spread",
            Action::ThinFarSide => "Thin far side",
            Action::ResetLadder => "Reset ladder",
            Action::ResetAllLadders => "Reset all ladders",
        }
    }

    /// Whether the action can run in `phase` on `cluster`.
    ///
    /// The cluster enters for one entry only, and it is the one the operator
    /// sees first on mainnet: the hosted explorer needs no validator, so a
    /// throttled endpoint (which polls as [`Phase::NoValidator`]) must not grey
    /// it out. Everything else turns purely on the phase.
    pub fn enabled(self, phase: Phase, cluster: Cluster) -> bool {
        match self {
            Action::Deploy => phase == Phase::ProgramAbsent,
            Action::InitRegistry => phase == Phase::RegistryAbsent,
            Action::CreateMarket => phase == Phase::MarketAbsent,
            Action::CreateVault => phase == Phase::VaultAbsent,
            Action::Deposit => phase == Phase::VaultUnseeded,
            Action::OpenExplorer => cluster.is_mainnet() || phase != Phase::NoValidator,
            // Self-deploys when the program is absent, so it's available the
            // moment a validator is up and runs until everything exists.
            Action::BootstrapAll => matches!(
                phase,
                Phase::ProgramAbsent
                    | Phase::RegistryAbsent
                    | Phase::MarketAbsent
                    | Phase::VaultAbsent
                    | Phase::VaultUnseeded
            ),
            // A take needs a live, seeded vault to match against.
            Action::ProbeSwap => phase == Phase::Ready,
            // Reclaim whatever exists from the program onward — program
            // rent, the registry + fee vault, and the market if present.
            Action::Teardown => matches!(
                phase,
                Phase::RegistryAbsent
                    | Phase::MarketAbsent
                    | Phase::VaultAbsent
                    | Phase::VaultUnseeded
                    | Phase::Ready
            ),
            Action::Wipe => true,
            // Moving a stake needs one to exist: a seeded vault. `Ready` is
            // every roster market at once; the prompt then re-reads the
            // selected market's vault fresh and refuses on its own terms.
            Action::LeaderDeposit | Action::LeaderWithdraw => phase == Phase::Ready,
            Action::RotateLeader => false,
            // The demo controls quote against a live vault.
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder
            | Action::ResetAllLadders => phase == Phase::Ready,
        }
    }

    /// Whether the action means anything on `cluster`.
    ///
    /// Orthogonal to [`Action::enabled`]: this asks whether the action has a
    /// counterpart on that chain at all, not whether the chain is currently in
    /// the right state for it. Everything is available on localnet — that is
    /// the throwaway ledger the panel was built to drive.
    pub fn available_on(self, cluster: Cluster) -> bool {
        if !cluster.is_mainnet() {
            return true;
        }
        match self {
            // Reading an account in the explorer writes nothing.
            Action::OpenExplorer => true,
            // Localnet-only by construction: these spawn a validator, deploy
            // through `anchor build`, airdrop the payer, or discard a ledger
            // that exists only on this machine. On mainnet the program is
            // published out of band and there is nothing to throw away.
            // "Bootstrap all" belongs here too: it mints mock pairs and funds
            // the committed taker, and on mainnet each step is worth watching.
            Action::Deploy | Action::BootstrapAll | Action::Wipe => false,
            // The ceremony. Mainnet-safe because each step re-reads the chain
            // and refuses when its account already exists, references the
            // real roster mints and never creates one, airdrops nothing, and
            // signs vault steps with the operator's leader rather than a
            // committed role key.
            Action::InitRegistry | Action::CreateMarket | Action::CreateVault | Action::Deposit => {
                true
            }
            // The leader's own stake. Mainnet-safe because the signer is the
            // operator's `--leader`, never a committed key; real mints are
            // balance-checked and never minted; and nothing sends until the
            // operator types `yes` over the exact amounts and bounds. The
            // reserved rotation is listed so it is visible — and stays greyed.
            Action::LeaderDeposit | Action::LeaderWithdraw | Action::RotateLeader => true,
            // Spends real money, signed by a committed taker role key that has
            // no mainnet counterpart.
            Action::ProbeSwap => false,
            // Closes every market, vault and the registry. On mainnet that
            // stays behind the dedicated headless binary and its typed
            // confirmation — never one keystroke in a panel.
            Action::Teardown => false,
            // The demo controls quote with committed localnet role keys, which
            // are not the leader of any real vault.
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder
            | Action::ResetAllLadders => false,
        }
    }

    /// Whether the action must wait for a verified chain identity (see
    /// [`crate::cluster::Identity`]). Everything that writes does. The two
    /// exceptions are the explorer, which only reads, and the wipe, which
    /// discards only this process's own ledger — the operator's way out when
    /// the identity check has refused everything else.
    pub fn needs_verified_chain(self) -> bool {
        !matches!(self, Action::OpenExplorer | Action::Wipe)
    }

    /// One-line reason the action is absent on `cluster` (only meaningful when
    /// [`Action::available_on`] is false).
    pub fn unavailable_reason(self, cluster: Cluster) -> &'static str {
        debug_assert!(!self.available_on(cluster));
        match self {
            Action::Deploy | Action::BootstrapAll | Action::Wipe => {
                "localnet only — no validator or ledger to drive on mainnet"
            }
            Action::ProbeSwap => "spends real funds — no mainnet taker key",
            Action::Teardown => "use the headless teardown binary on a real cluster",
            // Enumerated rather than a `_` arm, and that is the point:
            // `available_on` above is exhaustive, so adding a variant is a
            // compile error there and the author must decide its mainnet
            // answer. A wildcard here would quietly hand that new variant the
            // reason "localnet demo control" — a false statement to the
            // operator, and one no test could catch, since the reason test only
            // asserts the string is non-empty.
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder
            | Action::ResetAllLadders => "localnet demo control",
            // Available on mainnet, so only reachable by a misuse the debug
            // assertion above catches in test builds.
            Action::OpenExplorer
            | Action::InitRegistry
            | Action::CreateMarket
            | Action::CreateVault
            | Action::Deposit
            | Action::LeaderDeposit
            | Action::LeaderWithdraw
            | Action::RotateLeader => "",
        }
    }

    /// One-line reason the action is greyed out in `phase` on `cluster` (only
    /// meaningful when [`Action::enabled`] is false).
    ///
    /// Cluster-aware because the localnet wording is false on mainnet: there
    /// is no validator there by design, so "waiting for validator" would send
    /// the operator to wait for something that never arrives, and "deploy the
    /// program first" names an action mainnet does not offer.
    pub fn disabled_reason(self, phase: Phase, cluster: Cluster) -> &'static str {
        if phase == Phase::NoValidator {
            return if cluster.is_mainnet() {
                "endpoint not answering — check the RPC endpoint"
            } else {
                "waiting for validator"
            };
        }
        match self {
            Action::Deploy => "program already deployed",
            Action::InitRegistry if phase == Phase::ProgramAbsent && cluster.is_mainnet() => {
                "the program is not published on this cluster"
            }
            Action::InitRegistry if phase == Phase::ProgramAbsent => "deploy the program first",
            Action::InitRegistry => "registry already initialized",
            Action::CreateMarket if below(phase, Phase::MarketAbsent) => {
                "initialize the registry first"
            }
            Action::CreateMarket => "every roster market exists",
            Action::CreateVault if below(phase, Phase::VaultAbsent) => "create the markets first",
            Action::CreateVault => "every roster market has a vault",
            Action::Deposit if below(phase, Phase::VaultUnseeded) => "create the vaults first",
            Action::Deposit => "every vault is seeded",
            Action::BootstrapAll => "already bootstrapped",
            Action::ProbeSwap => "needs a live, seeded vault",
            Action::Teardown => "deploy the program first",
            Action::OpenExplorer | Action::Wipe => "",
            Action::LeaderDeposit | Action::LeaderWithdraw => "needs every vault seeded",
            Action::RotateLeader => "reserved — arrives with the leader-rotation program change",
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder
            | Action::ResetAllLadders => "needs a live, seeded vault",
        }
    }
}

/// `true` if `a` orders strictly before `b` in the bootstrap progression.
fn below(a: Phase, b: Phase) -> bool {
    fn rank(p: Phase) -> u8 {
        match p {
            Phase::NoValidator => 0,
            Phase::ProgramAbsent => 1,
            Phase::RegistryAbsent => 2,
            Phase::MarketAbsent => 3,
            Phase::VaultAbsent => 4,
            Phase::VaultUnseeded => 5,
            Phase::Ready => 6,
        }
    }
    rank(a) < rank(b)
}

/// The recommended next bootstrap step in `phase` on `cluster` — the first one
/// that is both enabled and offered there (mainnet never recommends a deploy).
pub fn recommended_next(phase: Phase, cluster: Cluster) -> Option<Action> {
    BOOTSTRAP
        .into_iter()
        .find(|a| a.available_on(cluster) && a.enabled(phase, cluster))
}

/// Owned context a background job needs. Cloned per dispatch so the job
/// thread owns everything it touches.
pub struct JobContext {
    pub rpc_url: String,
    /// Which chain this session drives. Carried here rather than re-derived
    /// from `rpc_url` because the two can disagree — a loopback URL may be
    /// forwarded to mainnet — and the operator's declared intent is what the
    /// gates must key on.
    pub cluster: Cluster,
    pub repo_root: PathBuf,
    pub wallet_path: String,
    pub wallet: Keypair,
    /// The operator-supplied vault leader (`--leader`), for a roster whose
    /// pairs name no leader file ([`market::LeaderKey::Operator`] — mainnet).
    /// `None` on localnet, whose pairs use the committed role keys.
    pub leader: Option<Keypair>,
    /// Lifecycle of the managed explorer container (an `explorer::state::*`
    /// value). The background starter and the "Open explorer" job both update
    /// it; the UI reads it; `App`'s `Drop` tears the container down unless it
    /// is `NO_DOCKER`.
    pub explorer_state: Arc<AtomicU8>,
    /// Serializes the explorer `docker compose up` so the background starter
    /// and "Open explorer" never run it concurrently.
    pub explorer_lock: Arc<Mutex<()>>,
}

impl JobContext {
    fn wallet(&self) -> Keypair {
        self.wallet.insecure_clone()
    }
}

/// Spawn the background job for `action`. [`Action::Wipe`] is handled by the
/// event loop instead (it mutates the owned validator), so it is a no-op
/// here. `selected` picks which discovered market the market-scoped actions
/// (the probe swap, the explorer targets) act on; `swap_units` is the
/// taker-selected notional (whole units of the input token) and `swap_side`
/// the direction a [`Action::ProbeSwap`] takes.
#[allow(clippy::too_many_arguments)]
pub fn dispatch(
    action: Action,
    ctx: &JobContext,
    state: &ChainState,
    tx: Sender<JobEvent>,
    selected: usize,
    swap_units: u64,
    swap_side: SwapSide,
    reshape_spread_bps: u32,
) {
    let rpc_url = ctx.rpc_url.clone();
    let repo_root = ctx.repo_root.clone();
    let wallet_path = ctx.wallet_path.clone();
    let wallet = ctx.wallet();
    let cluster = ctx.cluster;
    let operator_leader = ctx.leader.as_ref().map(Keypair::insecure_clone);
    let explorer_state = ctx.explorer_state.clone();
    let explorer_lock = ctx.explorer_lock.clone();
    // The market the market-scoped jobs target — resolved now, on the event
    // loop's fresh snapshot, so a job addresses the selected market and not
    // whichever the scan turns up first. The eCLOB controls also need its base
    // mint (to resolve the leader / quote authority + seed ladder) and the
    // sector index of its first live vault (the one to reprice / reshape).
    let selected_market = state.selected_market(selected);
    let target_market = selected_market.map(|m| m.address);
    let target_base_mint = selected_market.map(|m| m.base_mint);
    let target_vault = selected_market.and_then(|m| m.live_vaults.first().map(|(idx, _)| *idx));

    match action {
        Action::Deploy => {
            let pubkey = wallet.pubkey();
            job::spawn(tx, "Deploy", move |log| {
                deploy::deploy_program(log, &repo_root, &rpc_url, &wallet_path, &pubkey)
            });
        }
        Action::InitRegistry => {
            job::spawn(tx, "Init registry", move |log| {
                let client = chain::rpc(&rpc_url);
                do_init(&client, &wallet, cluster, log)?.one_shot()
            });
        }
        Action::CreateMarket => {
            job::spawn(tx, "Create markets", move |log| {
                let client = chain::rpc(&rpc_url);
                over_roster(cluster, "market", log, |config| {
                    do_create_market(&client, &wallet, &repo_root, cluster, config, log)
                })
            });
        }
        Action::CreateVault => {
            job::spawn(tx, "Create vaults", move |log| {
                let client = chain::rpc(&rpc_url);
                over_roster(cluster, "vault", log, |config| {
                    let leader = market::leader(&repo_root, config, operator_leader.as_ref())?;
                    do_create_vault(&client, &wallet, &repo_root, cluster, config, &leader, log)
                })
            });
        }
        Action::Deposit => {
            job::spawn(tx, "Seed deposits", move |log| {
                let client = chain::rpc(&rpc_url);
                // Sequential, so localnet's per-market quote top-up is safe (no
                // concurrent identical mints) — `MintBoth` lets each market
                // fund its own quote leg. Mainnet mints nothing.
                let funding = if cluster.is_mainnet() {
                    market::Funding::LeaderHeld
                } else {
                    market::Funding::MintBoth
                };
                over_roster(cluster, "deposit", log, |config| {
                    let leader = market::leader(&repo_root, config, operator_leader.as_ref())?;
                    do_deposit(
                        &client, &wallet, &repo_root, cluster, config, &leader, funding, log,
                    )
                })
            });
        }
        Action::BootstrapAll => {
            let pubkey = wallet.pubkey();
            let program_deployed = state.program_deployed;
            job::spawn(tx, "Bootstrap all", move |log| {
                // Deploy first if the program isn't on-chain yet, so a fresh
                // localnet bootstraps end-to-end from one action.
                if !program_deployed {
                    deploy::deploy_program(log, &repo_root, &rpc_url, &wallet_path, &pubkey)?;
                }
                let client = chain::rpc(&rpc_url);
                // Localnet only, and the code below relies on it: it iterates
                // the localnet `market::PAIRS` (every mint keypair-backed) and
                // mints both legs. The cluster gate keeps it off mainnet;
                // `seed_vault` would refuse a minting mode on a real mint
                // anyway.
                //
                // Sequential prelude — everything the parallel phase must not
                // race on. The registry and the shared USDC quote mint are
                // created once here (both create-once accounts). The shared
                // leader's USDC ATA is funded with the *whole* deposit total up
                // front, so the parallel phase can skip the per-market quote
                // top-up entirely — those top-ups are byte-identical across
                // markets and would collide on transaction signature if run
                // concurrently (and the pool would underflow as dedup drops
                // all but one). See `market::prefund_leader_quotes`.
                //
                // Each step is existence-checked, so a bootstrap resumed after a
                // partial run skips what already exists rather than failing on
                // it. (A resumed run does re-mint the prelude's quote pool; on a
                // throwaway ledger the surplus is harmless.)
                do_init(&client, &wallet, cluster, log)?.log_skip(log);
                market::ensure_quote_mints(&client, &wallet, &repo_root, log)?;
                market::prefund_leader_quotes(&client, &wallet, &repo_root, log)?;
                // Parallel phase: each pair's create_market → create_vault →
                // deposit is an independent chain against its own market PDA,
                // unique base mint, and unique amounts, so every market
                // pipelines against the one validator with no colliding
                // transactions (the shared quote leg was handled by the
                // prelude). `MintBaseOnly` tells `seed_vault` to skip that
                // shared top-up.
                //
                // Deliberately not a count: this said "the seven markets" and
                // went stale the moment the roster grew to nine. The roster
                // length is one `PAIRS.len()` away for anyone who needs it,
                // and the claim here is about independence, not arity.
                std::thread::scope(|scope| {
                    let workers: Vec<_> = market::PAIRS
                        .into_iter()
                        .map(|config| {
                            let rpc_url = rpc_url.clone();
                            let wallet = wallet.insecure_clone();
                            let repo_root = &repo_root;
                            let log = log.clone();
                            scope.spawn(move || -> Result<()> {
                                let client = chain::rpc(&rpc_url);
                                log.log(format!("— {} —", config.base.symbol));
                                let leader = market::leader(repo_root, config, None)?;
                                do_create_market(
                                    &client, &wallet, repo_root, cluster, config, &log,
                                )?
                                .log_skip(&log);
                                do_create_vault(
                                    &client, &wallet, repo_root, cluster, config, &leader, &log,
                                )?
                                .log_skip(&log);
                                do_deposit(
                                    &client,
                                    &wallet,
                                    repo_root,
                                    cluster,
                                    config,
                                    &leader,
                                    market::Funding::MintBaseOnly,
                                    &log,
                                )?
                                .log_skip(&log);
                                Ok(())
                            })
                        })
                        .collect();
                    for worker in workers {
                        worker
                            .join()
                            .map_err(|_| anyhow::anyhow!("a bootstrap worker panicked"))??;
                    }
                    Ok::<(), anyhow::Error>(())
                })?;
                // Fund the taker so a browser-wallet swap works out of the box.
                fund_taker(&client, &wallet, &repo_root, log)?;
                Ok(format!(
                    "Bootstrap complete — {} markets",
                    market::PAIRS.len()
                ))
            });
        }
        Action::ProbeSwap => {
            job::spawn(tx, "Probe swap", move |log| {
                let client = chain::rpc(&rpc_url);
                do_probe_swap(
                    &client,
                    &wallet,
                    &repo_root,
                    target_market,
                    swap_units,
                    swap_side,
                    log,
                )
            });
        }
        Action::Teardown => {
            job::spawn(tx, "Teardown", move |log| {
                let client = chain::rpc(&rpc_url);
                teardown::run(&client, &wallet, log)
            });
        }
        Action::OpenExplorer => {
            let targets = explorer_targets(state, selected);
            job::spawn(tx, "Open explorer", move |log| {
                // Mainnet short-circuits before the Docker block, and both
                // halves of that matter. The managed container indexes the
                // localnet, so bringing it up here would start a container the
                // entry banner promised this session would not — and would
                // overwrite the NO_DOCKER sentinel that stops `Drop for App`
                // tearing down a container it never owned. The URL then comes
                // from the one owner of that rule, which is what stops this arm
                // and `App::open_in_explorer` diverging again.
                if cluster.is_mainnet() {
                    // The endpoint is passed in deliberately rather than
                    // blanked: the router ignores it on mainnet, and its test
                    // pins that with a key-bearing URL, so this exercises the
                    // same path the test asserts on.
                    open_targets(log, &targets, |addr| {
                        explorer::account_url_for(cluster, addr, &rpc_url, false)
                    })?;
                    return Ok(format!(
                        "Opened {} account(s) in the hosted explorer (mainnet)",
                        targets.len()
                    ));
                }
                if !explorer::docker_available() {
                    log.log("Docker not found — opening the hosted explorer instead.");
                    log.log(
                        "Note: explorer.solana.com can't reach the localnet in Brave/Safari; \
                         install Docker for the local explorer, or open these links in \
                         Chrome/Firefox.",
                    );
                    open_targets(log, &targets, |addr| {
                        explorer::account_url_for(cluster, addr, &rpc_url, false)
                    })?;
                    return Ok(format!(
                        "Opened {} account(s) in the hosted explorer (fallback)",
                        targets.len()
                    ));
                }
                // Docker is present. Usually the background starter already
                // has it serving; if not, take the lock (waiting for any
                // in-flight start) and bring it up before opening.
                if explorer_state.load(Ordering::SeqCst) != explorer::state::READY {
                    let _guard = explorer_lock.lock().unwrap_or_else(|e| e.into_inner());
                    if explorer_state.load(Ordering::SeqCst) != explorer::state::READY {
                        explorer::ensure_running(log, &repo_root)?;
                        explorer_state.store(explorer::state::READY, Ordering::SeqCst);
                    }
                }
                open_targets(log, &targets, |addr| {
                    explorer::account_url_for(cluster, addr, &rpc_url, true)
                })?;
                Ok(format!(
                    "Opened {} account(s) in the local explorer",
                    targets.len()
                ))
            });
        }
        Action::RepegUp | Action::RepegDown => {
            let bps = if action == Action::RepegUp {
                REPEG_BPS
            } else {
                -REPEG_BPS
            };
            job::spawn(tx, "Re-peg", move |log| {
                let client = chain::rpc(&rpc_url);
                do_repeg(
                    &client,
                    &wallet,
                    &repo_root,
                    target_market,
                    target_base_mint,
                    target_vault,
                    bps,
                    log,
                )
            });
        }
        Action::WidenSpread | Action::TightenSpread | Action::ThinFarSide | Action::ResetLadder => {
            job::spawn(tx, "Reshape", move |log| {
                let client = chain::rpc(&rpc_url);
                do_reshape(
                    &client,
                    &wallet,
                    &repo_root,
                    target_market,
                    target_base_mint,
                    target_vault,
                    action,
                    reshape_spread_bps,
                    swap_side,
                    log,
                )
            });
        }
        Action::ResetAllLadders => {
            // Reset every market's first live vault, not just the selected one,
            // in one job — resolve each market's reshape target up front so the
            // job thread owns them (the same `(market, base_mint, vault_idx)`
            // triple `eclob_target` yields for a single market).
            let targets: Vec<(Pubkey, Pubkey, u32)> = state
                .markets
                .iter()
                .filter_map(|m| {
                    m.live_vaults
                        .first()
                        .map(|(idx, _)| (m.address, m.base_mint, *idx))
                })
                .collect();
            job::spawn(tx, "Reset all ladders", move |log| {
                let client = chain::rpc(&rpc_url);
                let total = targets.len();
                for (market, base_mint, vault_idx) in targets {
                    do_reshape(
                        &client,
                        &wallet,
                        &repo_root,
                        Some(market),
                        Some(base_mint),
                        Some(vault_idx),
                        Action::ResetLadder,
                        reshape_spread_bps,
                        swap_side,
                        log,
                    )?;
                }
                Ok(format!("Reset {total} ladders to the default shape"))
            });
        }
        // Wipe is handled by the event loop (owns the validator). The leader
        // commands open its amount → confirm prompt instead, and dispatch
        // through [`dispatch_leader`] once the operator types `yes`; the
        // reserved rotation is never enabled.
        Action::Wipe | Action::LeaderDeposit | Action::LeaderWithdraw | Action::RotateLeader => {}
    }
}

/// Spawn the job that sends a confirmed leader `ticket`. The leader key is
/// resolved here, per pair, exactly as the ceremony's vault steps resolve it:
/// the committed role key on localnet, the operator's `--leader` on mainnet.
pub fn dispatch_leader(ctx: &JobContext, ticket: crate::leader::Ticket, tx: Sender<JobEvent>) {
    let rpc_url = ctx.rpc_url.clone();
    let repo_root = ctx.repo_root.clone();
    let wallet = ctx.wallet();
    let cluster = ctx.cluster;
    let operator_leader = ctx.leader.as_ref().map(Keypair::insecure_clone);
    job::spawn(tx, ticket.op.label(), move |log| {
        let client = chain::rpc(&rpc_url);
        let config = market::config_for(&repo_root, cluster, &ticket.market.base_mint)
            .context("the selected market is not in this cluster's roster")?;
        let leader = market::leader(&repo_root, config, operator_leader.as_ref())?;
        crate::leader::execute(&client, &wallet, &leader, &repo_root, cluster, &ticket, log)
    });
}

/// Resolve the selected market's `(address, base_mint, vault_idx)` for an
/// eCLOB control, erroring with a demo-friendly message when no live vault is
/// selected. Shared by [`do_repeg`] and [`do_reshape`].
fn eclob_target(
    market: Option<Pubkey>,
    base_mint: Option<Pubkey>,
    vault_idx: Option<u32>,
) -> Result<(Pubkey, Pubkey, u32)> {
    let market = market.context("no market selected")?;
    let base_mint = base_mint.context("no market selected")?;
    let vault_idx = vault_idx.context("no live vault on the selected market")?;
    Ok((market, base_mint, vault_idx))
}

/// Reprice the selected market's vault (`set_reference_price`) — the cheap
/// hot path. Reads the live reference, scales it by `bps`, and re-stamps it at
/// the current slot, moving the *whole* ladder without reshaping it. The
/// leader (quote authority) co-signs; the admin wallet pays the fee.
#[allow(clippy::too_many_arguments)]
fn do_repeg(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    market: Option<Pubkey>,
    base_mint: Option<Pubkey>,
    vault_idx: Option<u32>,
    bps: f64,
    log: &Logger,
) -> Result<String> {
    let (market, base_mint, vault_idx) = eclob_target(market, base_mint, vault_idx)?;
    let leader = market::leader_for(repo_root, &base_mint)?;
    // Bump the live reference — decode it to its atoms-ratio, scale by the bps
    // step, and re-encode — so the nudge is relative to the current peg.
    let current = accounts::read_reference_price(client, &market, vault_idx)
        .context("read current reference price")?;
    // A dark market has no peg to nudge. That is the ordinary pre-bot state,
    // not a corrupt one — the bootstrap leaves the reference unstamped (see
    // `market::seed_vault`) and the maker's startup invalidation re-darkens a
    // market whose quotes have aged out. Name it here: scaling a zero-or-
    // sentinel reference otherwise fails further down as an "atoms-ratio 0"
    // encode error that points at nothing. `is_matchable` is the engine's own
    // predicate, so this reads dark exactly when the matcher does.
    if !current.is_matchable() {
        anyhow::bail!("market is dark — start its maker bot before re-pegging");
    }
    let ratio = current.quote_for_base(PRICE_SCALE) as f64 / PRICE_SCALE as f64;
    let bumped = ratio * (1.0 + bps / 10_000.0);
    let price =
        Price::from_value(bumped).with_context(|| format!("re-peg to atoms-ratio {bumped}"))?;
    let slot = client.get_slot().context("current slot")?;
    // A re-peg is a fresh quote, so it re-stamps the wall-clock datum too
    // — otherwise the new price would inherit the old quote's remaining
    // level life.
    let ix = set_reference_price_ix(
        leader.pubkey(),
        market,
        vault_idx,
        price,
        slot,
        dropset_sdk::time::now_unix(),
    );
    chain::send_logged(
        client,
        wallet,
        &[wallet, &leader],
        &[ix],
        "set_reference_price",
        log,
    )
    .context("set_reference_price")?;
    log.accounts_changed();
    // Report the concrete new reference (human quote-per-base) so the green
    // success line makes the repeg's effect obvious — the atoms-ratio scales
    // back by the pair's decimal gap.
    let human = market::config_for(repo_root, Cluster::Localnet, &base_mint)
        .map(|c| atoms_ratio_to_human(bumped, c.base.decimals, c.quote.decimals));
    Ok(match human {
        Some(p) => format!(
            "Re-pegged {bps:+} bps \u{2192} reference now {} \u{2014} whole book shifts",
            crate::book::fmt_price(p)
        ),
        None => format!("Re-pegged {bps:+} bps \u{2014} whole book shifts"),
    })
}

/// Reshape the selected market's ladder (`set_liquidity_profile`) — the cold
/// path. Rewrites the quote profile (spread / per-side depth) while the peg
/// stays put, so the book's *shape* changes without moving the anchor. The
/// preset is chosen by `action`; the leader co-signs, the admin wallet pays.
/// `swap_side` orients the [`Action::ThinFarSide`] preset — the "far" side is
/// the one the current swap would take from (ask on a Buy, bid on a Sell), so
/// flipping the swap side (`S`) flips which ladder thins.
#[allow(clippy::too_many_arguments)]
fn do_reshape(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    market: Option<Pubkey>,
    base_mint: Option<Pubkey>,
    vault_idx: Option<u32>,
    action: Action,
    spread_bps: u32,
    swap_side: SwapSide,
    log: &Logger,
) -> Result<String> {
    let (market, base_mint, vault_idx) = eclob_target(market, base_mint, vault_idx)?;
    // Localnet only — the cluster gate keeps the eCLOB controls off mainnet,
    // so the committed roster and its role keys are the right ones here.
    let config = market::config_for(repo_root, Cluster::Localnet, &base_mint)
        .context("market not in the bootstrap roster")?;
    let leader = market::leader(repo_root, config, None)?;
    // Widen / tighten step the spread by ±5 bps (the caller adjusts `spread_bps`
    // before dispatch); the ladder is rebuilt at that spread, keeping all four
    // levels. Thin-far-side keeps the full bid ladder over a depth-scaled ask
    // ladder at the same spread; reset returns to the default spread.
    let (bytes, summary) = match action {
        Action::WidenSpread | Action::TightenSpread => (
            market::ladder_profile_bytes(&market::ladder_at_spread_bps(spread_bps), NEVER_EXPIRES),
            format!("Spread now {spread_bps} bps — multi-level ladder, peg unchanged"),
        ),
        Action::ThinFarSide => {
            let full = market::ladder_at_spread_bps(spread_bps);
            let mut thinned = full;
            for (_, size_bps) in &mut thinned {
                *size_bps = (f64::from(*size_bps) * THIN_DEPTH_SCALE).round() as u16;
            }
            // Thin the far side relative to the current swap: a Buy lifts asks,
            // so its far side is the ask ladder; a Sell hits bids, so its far
            // side is the bid ladder. The near side keeps the full ladder.
            let (bids, asks, side) = match swap_side {
                SwapSide::Buy => (full, thinned, "ask"),
                SwapSide::Sell => (thinned, full, "bid"),
            };
            (
                market::ladder_profile_bytes_asym(&bids, &asks, NEVER_EXPIRES),
                format!("Thinned the far ({side}) side — that depth shrinks, peg unchanged"),
            )
        }
        Action::ResetLadder => (
            market::ladder_profile_bytes(
                &market::ladder_at_spread_bps(market::DEFAULT_SPREAD_BPS),
                config.expiry_offset_secs,
            ),
            format!(
                "Reset to the default {}-bps multi-level ladder",
                market::DEFAULT_SPREAD_BPS
            ),
        ),
        other => unreachable!("do_reshape received a non-reshape action: {other:?}"),
    };
    let ix = set_liquidity_profile_ix(leader.pubkey(), market, vault_idx, bytes);
    chain::send_logged(
        client,
        wallet,
        &[wallet, &leader],
        &[ix],
        "set_liquidity_profile",
        log,
    )
    .context("set_liquidity_profile")?;
    log.accounts_changed();
    Ok(summary)
}

/// `(label, address)` pairs to open in the explorer for the current state —
/// the program, the registry, and the *selected* market's accounts.
fn explorer_targets(state: &ChainState, selected: usize) -> Vec<(&'static str, Pubkey)> {
    let mut targets = vec![("program", dropset_sdk::DROPSET_ID)];
    if let Some(reg) = &state.registry {
        targets.push(("registry", reg.address));
        targets.push(("registry fee vault", reg.fee_vault));
    }
    if let Some(mkt) = state.selected_market(selected) {
        targets.push(("market", mkt.address));
        targets.push(("base treasury", mkt.base_treasury));
        targets.push(("quote treasury", mkt.quote_treasury));
    }
    targets
}

/// Open each `(label, address)` target in the browser, building its URL with
/// `url_for`. Logs each as it goes; the first failure aborts.
fn open_targets(
    log: &Logger,
    targets: &[(&'static str, Pubkey)],
    url_for: impl Fn(&Pubkey) -> String,
) -> Result<()> {
    for (label, addr) in targets {
        log.log(format!("Opening {label} {addr}"));
        open::that(url_for(addr)).with_context(|| format!("open {label}"))?;
    }
    Ok(())
}

/// The least SOL the mainnet payer must hold before a ceremony step sends. A
/// floor that catches an empty or wrong wallet before the first send, not a
/// cost estimate: a step that needs more still fails cleanly at the send.
const MAINNET_MIN_PAYER_LAMPORTS: u64 = LAMPORTS_PER_SOL / 10;

/// Make sure `wallet` can pay for what follows — admin paths waive the
/// program's fees, but rent and transaction fees still cost lamports.
///
/// On localnet that means airdropping a working balance when it runs low. On
/// mainnet there is no faucet, so it checks the balance against
/// [`MAINNET_MIN_PAYER_LAMPORTS`] and refuses below it; a failed balance read
/// refuses too, rather than reading as zero or as plenty.
fn ensure_funded(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Pubkey,
    cluster: Cluster,
    log: &Logger,
) -> Result<()> {
    if cluster.is_mainnet() {
        let balance = client
            .get_balance(wallet)
            .map_err(|_| anyhow::anyhow!("could not read the payer's balance — refusing"))?;
        if balance < MAINNET_MIN_PAYER_LAMPORTS {
            anyhow::bail!(
                "payer {wallet} holds {balance} lamports, below the {MAINNET_MIN_PAYER_LAMPORTS} \
                 floor — fund it first; nothing was sent"
            );
        }
        return Ok(());
    }
    let balance = client.get_balance(wallet).unwrap_or(0);
    if balance < LAMPORTS_PER_SOL {
        log.log("Airdropping working balance to the wallet…");
        if let Err(e) = chain::airdrop(client, wallet, 100 * LAMPORTS_PER_SOL) {
            log.log(format!("airdrop warning: {e:#}"));
        }
    }
    Ok(())
}

/// Fund the taker (`FFFF`) at bootstrap so importing it into a browser wallet
/// (Phantom on `localhost:8899`) lands on spendable balances immediately —
/// SOL for fees, plus every seeded market's base mint and the shared quote
/// (USDC), at the same per-side amounts a vault opens with. Runs sequentially
/// after the parallel seed phase, so the shared-USDC top-up is a single summed
/// mint rather than a byte-identical race across markets. Localnet-only: it
/// mints mock tokens whose authority is the admin wallet.
fn fund_taker(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    log: &Logger,
) -> Result<()> {
    let taker = market::taker(repo_root)?.pubkey();
    ensure_funded(client, &taker, Cluster::Localnet, log)?;
    log.log(format!("Funding taker {taker} for wallet swaps…"));
    // Base leg per market; accumulate the shared quote to mint once at the end.
    let mut total_quote: u64 = 0;
    let mut shared_quote_mint = None;
    for config in market::PAIRS {
        let (base_mint, quote_mint) = market::pair_mints(repo_root, config)?;
        let (base_atoms, quote_atoms) = market::seed_deposit(config);
        let base_ata = chain::create_ata_idempotent(client, wallet, &taker, &base_mint)
            .context("taker base ATA")?;
        chain::mint_to(client, wallet, &base_mint, &base_ata, base_atoms)
            .context("fund taker base leg")?;
        total_quote = total_quote.saturating_add(quote_atoms);
        shared_quote_mint = Some(quote_mint);
    }
    if let Some(quote_mint) = shared_quote_mint {
        let quote_ata = chain::create_ata_idempotent(client, wallet, &taker, &quote_mint)
            .context("taker quote ATA")?;
        chain::mint_to(client, wallet, &quote_mint, &quote_ata, total_quote)
            .context("fund taker quote leg")?;
    }
    Ok(())
}

/// What a ceremony step found when it read the chain fresh.
///
/// The step itself only reports; what an existing account *means* is the
/// caller's call. A numbered menu step is one-shot, so for it an existing
/// account is a refusal (see [`Outcome::one_shot`]) — the operator asked to
/// create something that is already there, and a red line says so. The
/// roster loops and "Bootstrap all" are resumable, so for them it is a skip.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// The step sent its transaction(s); the summary says what landed.
    Done(String),
    /// The account the step would create already exists, so nothing was sent.
    AlreadyThere(String),
}

impl Outcome {
    /// A one-shot command's reading: `AlreadyThere` is a refusal, so the job
    /// ends red and names what exists, rather than reporting success for a
    /// command that did nothing.
    fn one_shot(self) -> Result<String> {
        match self {
            Outcome::Done(summary) => Ok(summary),
            Outcome::AlreadyThere(what) => {
                anyhow::bail!("refused — {what}; nothing was sent")
            }
        }
    }

    /// A resumable caller's reading: log an `AlreadyThere` and carry on.
    fn log_skip(self, log: &Logger) {
        if let Outcome::AlreadyThere(what) = self {
            log.log(format!("skip — {what}"));
        }
    }
}

/// Run `step` over every `cluster` roster pair, in order, and summarize.
///
/// Resumable by construction: a pair whose account already exists is skipped
/// and logged, so re-running after a partial failure finishes the job rather
/// than tripping over the part that landed. The first real error stops the
/// loop. If **every** pair was already there, the command as a whole did
/// nothing, and that is reported as the one-shot refusal it is.
fn over_roster(
    cluster: Cluster,
    noun: &str,
    log: &Logger,
    mut step: impl FnMut(&PairConfig) -> Result<Outcome>,
) -> Result<String> {
    let roster = market::roster(cluster);
    let mut created = 0usize;
    for config in roster {
        log.log(format!("— {} —", config.base.symbol));
        match step(config).with_context(|| format!("{} {noun}", config.base.symbol))? {
            Outcome::Done(summary) => {
                log.log(summary);
                created += 1;
            }
            skipped @ Outcome::AlreadyThere(_) => skipped.log_skip(log),
        }
    }
    let total = roster.len();
    if created == 0 {
        return Outcome::AlreadyThere(format!("every roster {noun} ({total}) already exists"))
            .one_shot();
    }
    Ok(format!(
        "{created} {noun}(s) done, {} already on-chain",
        total - created
    ))
}

/// Create the registry: resolve the fee mint, then send `init` (genesis
/// admin = wallet, which must equal the program's upgrade authority).
///
/// Refuses (`AlreadyThere`) when the registry exists. The program's own `init`
/// constraint would reject a second registry too, but only after the mock fee
/// mint below had been created and paid for — so the check is here, first.
///
/// The fee mint is per cluster. Localnet mints a throwaway mock (its address
/// is read back from the registry, so it never needs to be stable). Mainnet
/// charges the per-vault fee in real USDC, verified to exist and never created.
fn do_init(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    cluster: Cluster,
    log: &Logger,
) -> Result<Outcome> {
    if accounts::registry_fresh(client)?.is_some() {
        return Ok(Outcome::AlreadyThere(
            "the registry is already initialized".into(),
        ));
    }
    ensure_funded(client, &wallet.pubkey(), cluster, log)?;
    let fee_mint = if cluster.is_mainnet() {
        chain::verify_mint(client, &market::MAINNET_USDC, 6).context("verify the USDC fee mint")?;
        market::MAINNET_USDC
    } else {
        log.log("Creating mock fee mint…");
        chain::create_spl_mint(client, wallet).context("create fee mint")?
    };
    log.log(format!("fee mint: {fee_mint}"));
    let ix = chain::build_init_ix(&wallet.pubkey(), &fee_mint);
    // Trailing rent top-up for the registry PDA — see RENT_TOPUP_LAMPORTS.
    let topup = chain::system_transfer_ix(
        &wallet.pubkey(),
        &chain::registry_pda(),
        chain::RENT_TOPUP_LAMPORTS,
    );
    chain::send_logged(client, wallet, &[wallet], &[ix, topup], "init", log)
        .context("send init")?;
    log.accounts_changed();
    Ok(Outcome::Done("Registry initialized".into()))
}

/// Create `config`'s market: make its mints usable, then `create_market`
/// charged (and waived, admin) against the registry's stamped fee mint.
///
/// Refuses (`AlreadyThere`) when the market PDA exists, checked **before**
/// anything else so a repeat creates no mints either. On mainnet the mints are
/// only verified, never created (see [`market::ensure_pair_mints`]).
fn do_create_market(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    config: &PairConfig,
    log: &Logger,
) -> Result<Outcome> {
    let registry = accounts::registry_fresh(client)?.context("registry not found — init first")?;
    let (base_mint, quote_mint) = market::pair_mints(repo_root, config)?;
    let market_address = chain::market_pda(&base_mint, &quote_mint);
    if chain::fetch_fresh(client, &market_address, "market")?.is_some() {
        return Ok(Outcome::AlreadyThere(format!(
            "the {} market already exists ({market_address})",
            config.base.symbol
        )));
    }
    ensure_funded(client, &wallet.pubkey(), cluster, log)?;
    let (base_mint, quote_mint) =
        market::ensure_pair_mints(client, wallet, repo_root, config, log)?;
    // A distinct (never-read, admin path) fee source — must not alias the
    // payer, or anchor-v2 rejects it as a duplicate mutable account.
    let fee_source = Keypair::new().pubkey();
    let ix = chain::build_create_market_ix(
        &wallet.pubkey(),
        &fee_source,
        &base_mint,
        &quote_mint,
        &registry.fee_mint,
        &registry.fee_token_program,
    );
    // Trailing rent top-up for the market PDA — see RENT_TOPUP_LAMPORTS.
    let topup = chain::system_transfer_ix(
        &wallet.pubkey(),
        &chain::market_pda(&base_mint, &quote_mint),
        chain::RENT_TOPUP_LAMPORTS,
    );
    chain::send_logged(
        client,
        wallet,
        &[wallet],
        &[ix, topup],
        "create_market",
        log,
    )
    .context("send create_market")?;
    log.accounts_changed();
    Ok(Outcome::Done(format!(
        "{} market created",
        config.base.symbol
    )))
}

/// Open `leader`'s vault on `config`'s market via the admin path. The vault is
/// left empty; [`do_deposit`] seeds it.
///
/// **Never opens a second vault for the same leader.** The leader's vaults are
/// read fresh from the market slab first, and any at all is `AlreadyThere` —
/// on a retry, after a crash, or against a stale poll alike. The program does
/// not enforce one-vault-per-leader today, so this check is the only thing
/// standing between a double keypress and a second vault; when a program-side
/// guard lands, the two agree by construction (both key on the leader).
///
/// The leader must differ from the admin `wallet`: anchor-v2 rejects the same
/// key in the admin and leader slots, and admin teardown's
/// `force_withdraw_leader` would alias them. The fee source likewise must
/// differ from the payer.
#[allow(clippy::too_many_arguments)]
fn do_create_vault(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    config: &PairConfig,
    leader: &Keypair,
    log: &Logger,
) -> Result<Outcome> {
    if leader.pubkey() == wallet.pubkey() {
        anyhow::bail!(
            "the leader must not be the admin wallet ({})",
            wallet.pubkey()
        );
    }
    let registry = accounts::registry_fresh(client)?.context("registry not found — init first")?;
    // Address this config's own market by its PDA — the roster brings up many
    // markets, so the first-found one in `ChainState` isn't necessarily this
    // pair's.
    let (base_mint, quote_mint) = market::pair_mints(repo_root, config)?;
    let market_address = chain::market_pda(&base_mint, &quote_mint);
    let seats = accounts::vault_seats_fresh(client, &market_address)?
        .context("market not found — create the market first")?;
    if let Some(seat) = existing_seat(&seats, &leader.pubkey()) {
        return Ok(Outcome::AlreadyThere(format!(
            "leader {} already leads vault #{} on the {} market — never opening a second",
            leader.pubkey(),
            seat.seq,
            config.base.symbol
        )));
    }
    ensure_funded(client, &wallet.pubkey(), cluster, log)?;
    let fee_source = Keypair::new().pubkey();
    log.log(format!("vault leader: {}", leader.pubkey()));
    let ix = chain::build_create_vault_ix(
        &wallet.pubkey(),
        &fee_source,
        &market_address,
        &registry.fee_mint,
        &registry.fee_token_program,
        &leader.pubkey(),
    );
    // Trailing rent top-up for the market PDA, which create_vault grows —
    // see RENT_TOPUP_LAMPORTS.
    let topup = chain::system_transfer_ix(
        &wallet.pubkey(),
        &market_address,
        chain::RENT_TOPUP_LAMPORTS,
    );
    chain::send_logged(client, wallet, &[wallet], &[ix, topup], "create_vault", log)
        .context("send create_vault")?;
    log.accounts_changed();
    Ok(Outcome::Done(format!(
        "{} vault created — empty until seeded",
        config.base.symbol
    )))
}

/// The leader's existing vault among `seats`, if any — what makes
/// [`do_create_vault`] refuse. Any vault at all counts: one is the most a
/// leader may lead.
fn existing_seat(seats: &[VaultSeat], leader: &Pubkey) -> Option<VaultSeat> {
    accounts::seats_led_by(seats, leader).first().copied()
}

/// What [`do_deposit`] found for the leader among a market's vaults.
#[derive(Debug, PartialEq, Eq)]
enum DepositSeat {
    /// Exactly one vault, empty — the one to seed.
    Fund(VaultSeat),
    /// Exactly one vault, already holding a deposit — nothing to do.
    Seeded(VaultSeat),
    /// No vault for this leader — create it first.
    NoVault,
    /// More than one — ambiguous, and guessing which to fund is not this
    /// step's call.
    Ambiguous(usize),
}

/// Classify the leader's vaults on a market for the deposit step.
fn deposit_seat(seats: &[VaultSeat], leader: &Pubkey) -> DepositSeat {
    match accounts::seats_led_by(seats, leader)[..] {
        [] => DepositSeat::NoVault,
        [seat] if seat.seeded => DepositSeat::Seeded(seat),
        [seat] => DepositSeat::Fund(seat),
        ref many => DepositSeat::Ambiguous(many.len()),
    }
}

/// Seed `leader`'s vault on `config`'s market: set the quote ladder and make
/// the opening deposit (see [`market::seed_vault`]). The market stays **dark**
/// until a maker bot quotes it — `seed_vault` documents why that is the correct
/// opening state.
///
/// Read fresh, and classified by [`deposit_seat`]: the leader's vault is
/// located in the market slab **by leader**, never by an assumed sector index,
/// and a vault that already holds a deposit is `AlreadyThere`, so a retry never
/// deposits on top of a deposit. Note the bound: "holds a deposit" is read off
/// the vault's shares and inventory, so a vault later drained back to empty
/// reads as unseeded and may be seeded again — deliberately, since it is then
/// an empty vault like any other. A leader with no vault, or with more than
/// one, is refused outright.
#[allow(clippy::too_many_arguments)]
fn do_deposit(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    cluster: Cluster,
    config: &PairConfig,
    leader: &Keypair,
    funding: market::Funding,
    log: &Logger,
) -> Result<Outcome> {
    let (base_mint, quote_mint) = market::pair_mints(repo_root, config)?;
    let market_address = chain::market_pda(&base_mint, &quote_mint);
    let seats = accounts::vault_seats_fresh(client, &market_address)?
        .context("market not found — create the market first")?;
    let seat = match deposit_seat(&seats, &leader.pubkey()) {
        DepositSeat::Fund(seat) => seat,
        DepositSeat::Seeded(seat) => {
            return Ok(Outcome::AlreadyThere(format!(
                "the {} vault #{} already holds a deposit",
                config.base.symbol, seat.seq
            )))
        }
        DepositSeat::NoVault => anyhow::bail!(
            "leader {} leads no vault on the {} market — create the vault first",
            leader.pubkey(),
            config.base.symbol
        ),
        DepositSeat::Ambiguous(n) => anyhow::bail!(
            "leader {} leads {n} vaults on the {} market — refusing to guess which to seed",
            leader.pubkey(),
            config.base.symbol
        ),
    };
    ensure_funded(client, &wallet.pubkey(), cluster, log)?;
    // The treasuries and decimals `seed_vault` needs. A failed read here is
    // after the existence check, so it can only abort, never double-send.
    let market = accounts::read_market_at(client, market_address)
        .context("could not read the market back — refusing")?;
    market::seed_vault(
        client, wallet, leader, config, &market, seat.idx, funding, log,
    )?;
    log.accounts_changed();
    Ok(Outcome::Done(format!(
        "{} vault seeded — dark until a maker bot quotes it",
        config.base.symbol
    )))
}

/// Whole units of the input token a swap probe spends by default — scaled to
/// atoms by that leg's mint decimals at send time (quote on a Buy, base on a
/// Sell). The TUI seeds its editable swap amount with this; the taker overrides
/// it via the amount input (`a`).
pub const DEFAULT_PROBE_UNITS: u64 = 10;

/// Exercise — and measure the CU of — the swap path with a small taker take
/// against the seeded vault, on `side` (a Buy pays quote / receives base, a
/// Sell pays base / receives quote). The swapper is the dedicated `FFFF` taker
/// role key, never the admin: it signs and pays for the take, so the probe
/// exercises a real third-party taker against the bot's quotes rather than
/// the admin trading with itself. The admin stays the mint authority — it
/// funds the taker's input leg and creates its ATAs, but takes no part in the
/// swap transaction. The realized CU lands in the CU pane under "swap" via
/// [`chain::send_logged`]; depth and balances refresh after.
fn do_probe_swap(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    target: Option<Pubkey>,
    units: u64,
    side: SwapSide,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), Cluster::Localnet, log)?;
    // Swap against the selected market when one is set. The TUI always has a
    // selection, so the fallback is for a caller that supplies none.
    //
    // That fallback takes the **lowest market address**, not the first row of
    // the operator's list: no symbol map is passed here, so the sort has
    // nothing to order by and falls back to its address tiebreak. This
    // comment used to claim the fallback matched the book the operator was
    // looking at, which was never true — before the list was sorted at all it
    // was simply whichever market the account scan happened to yield first.
    // Deterministic and stated beats arbitrary and mis-stated; wire the map
    // through if the two ever need to agree.
    let market = match target {
        Some(address) => accounts::read_market_at(client, address),
        None => accounts::poll(client, &wallet.pubkey(), None, 0, &[])
            .markets
            .into_iter()
            .next(),
    }
    .context("no market — bootstrap first")?;
    if market.active_count == 0 {
        anyhow::bail!("no live vault to swap against — create the vault first");
    }
    // A seeded vault is not a matchable one: markets open dark and only quote
    // once a maker bot stamps a reference. Without this the probe sends, the
    // matcher skips the unpriced vault, and a zero-fill take reports back as
    // "filled" — `MarketView::reference_price` is already `None` for an unset
    // or sentinel reference, so the poll answers it without another read.
    if market.reference_price.is_none() {
        anyhow::bail!("market is dark — start its maker bot before probing a swap");
    }

    // The taker is FFFF — fund it with SOL so it pays its own fee, and give it
    // both ATAs (admin is the mint authority): one holds the leg it pays, the
    // other receives the leg it gets. Which is which flips with the side.
    let taker = market::taker(repo_root)?;
    let taker_pk = taker.pubkey();
    ensure_funded(client, &taker_pk, Cluster::Localnet, log)?;
    let quote_ata = chain::create_ata_idempotent(client, wallet, &taker_pk, &market.quote_mint)
        .context("taker quote ATA")?;
    let base_ata = chain::create_ata_idempotent(client, wallet, &taker_pk, &market.base_mint)
        .context("taker base ATA")?;

    // Fund the input leg the take spends — quote on a Buy, base on a Sell, each
    // scaled to atoms by its mint's decimals. `limit_price_bits` disables the
    // price bound in the take's favor (INFINITY = no ceiling for a Buy, ZERO =
    // no floor for a Sell), since the probe accepts any fill.
    let (input_mint, input_ata, input_decimals, limit_price) = match side {
        SwapSide::Buy => (
            market.quote_mint,
            quote_ata,
            market.quote_decimals,
            Price::INFINITY,
        ),
        SwapSide::Sell => (
            market.base_mint,
            base_ata,
            market.base_decimals,
            Price::ZERO,
        ),
    };
    let notional = 10u64.pow(input_decimals as u32).saturating_mul(units);
    chain::mint_to(client, wallet, &input_mint, &input_ata, notional)
        .context("fund taker input leg")?;

    let verb = match side {
        SwapSide::Buy => "buys",
        SwapSide::Sell => "sells",
    };
    log.log(format!("probe swap: {taker_pk} {verb} with {units} units"));
    let ix = chain::build_swap_ix(
        &taker_pk,
        &market.address,
        &market.base_mint,
        &market.quote_mint,
        &market.base_treasury,
        &market.quote_treasury,
        side as u8,
        notional,
        limit_price.as_u32(),
        0, // accept any output (probe)
    );
    chain::send_logged(client, &taker, &[&taker], &[ix], "swap", log).context("swap")?;
    log.accounts_changed();
    Ok("Swap probe filled — see the CU pane".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHASES: [Phase; 7] = [
        Phase::NoValidator,
        Phase::ProgramAbsent,
        Phase::RegistryAbsent,
        Phase::MarketAbsent,
        Phase::VaultAbsent,
        Phase::VaultUnseeded,
        Phase::Ready,
    ];

    const L: Cluster = Cluster::Localnet;
    const M: Cluster = Cluster::Mainnet;

    /// Every `Action`, so a cluster-gate test covers the demo controls too —
    /// those are reachable only by keybinds, never appear in `MENU`, and so
    /// would otherwise go unchecked.
    ///
    /// Note the bound: this is every **`Action`**, not everything a keystroke
    /// can reach. The maker/taker bot toggles are not `Action`s and are gated
    /// separately in `App` (see `App::refuse_on_mainnet`); an earlier version
    /// of this comment claimed the wider scope and was wrong.
    ///
    /// Kept complete by [`all_actions_lists_every_variant`], not by care — the
    /// `[Action; 17]` length annotation does not change when a variant is
    /// added, so nothing else would notice the list going stale.
    const ALL_ACTIONS: [Action; 20] = [
        Action::Deploy,
        Action::InitRegistry,
        Action::CreateMarket,
        Action::CreateVault,
        Action::Deposit,
        Action::OpenExplorer,
        Action::BootstrapAll,
        Action::ProbeSwap,
        Action::Teardown,
        Action::Wipe,
        Action::LeaderDeposit,
        Action::LeaderWithdraw,
        Action::RotateLeader,
        Action::RepegUp,
        Action::RepegDown,
        Action::WidenSpread,
        Action::TightenSpread,
        Action::ThinFarSide,
        Action::ResetLadder,
        Action::ResetAllLadders,
    ];

    #[test]
    fn every_action_is_available_on_localnet() {
        // Localnet is the throwaway ledger the panel exists to drive, so the
        // cluster gate must never be what hides something there.
        for a in ALL_ACTIONS {
            assert!(a.available_on(Cluster::Localnet), "{:?}", a.label());
        }
    }

    #[test]
    fn mainnet_exposes_only_the_ceremony_and_the_confirm_gated_leader_stake() {
        // The load-bearing assertion of the whole mode: on mainnet the only
        // reachable writes are the four one-shot ceremony steps and the
        // leader's own stake, which sends only after a typed `yes` over the
        // exact amounts (the reserved rotation is listed but never enabled).
        // A later change that exposes anything else — a mock-minting
        // bootstrap, a committed-key demo control — fails here.
        let ceremony = [
            Action::InitRegistry,
            Action::CreateMarket,
            Action::CreateVault,
            Action::Deposit,
        ];
        let leader_stake = [
            Action::LeaderDeposit,
            Action::LeaderWithdraw,
            Action::RotateLeader,
        ];
        for a in ALL_ACTIONS {
            let expected =
                a == Action::OpenExplorer || ceremony.contains(&a) || leader_stake.contains(&a);
            assert_eq!(
                a.available_on(M),
                expected,
                "{:?} mainnet availability",
                a.label()
            );
        }
    }

    #[test]
    fn mainnet_explorer_survives_an_unresponsive_endpoint() {
        // A throttled endpoint polls as NoValidator. The hosted explorer needs
        // no validator, so it must stay usable — on localnet it still waits.
        assert!(Action::OpenExplorer.enabled(Phase::NoValidator, M));
        assert!(!Action::OpenExplorer.enabled(Phase::NoValidator, L));
    }

    #[test]
    fn disabled_reasons_name_the_cluster_truthfully() {
        // "waiting for validator" is false on mainnet, where there is none.
        assert_eq!(
            Action::InitRegistry.disabled_reason(Phase::NoValidator, L),
            "waiting for validator"
        );
        let mainnet = Action::InitRegistry.disabled_reason(Phase::NoValidator, M);
        assert!(!mainnet.contains("validator"), "{mainnet}");
        // And mainnet never tells the operator to deploy.
        let absent = Action::InitRegistry.disabled_reason(Phase::ProgramAbsent, M);
        assert!(!absent.contains("deploy"), "{absent}");
        assert_eq!(
            Action::InitRegistry.disabled_reason(Phase::ProgramAbsent, L),
            "deploy the program first"
        );
    }

    #[test]
    fn mainnet_never_recommends_a_localnet_step() {
        assert_eq!(recommended_next(Phase::ProgramAbsent, M), None);
        assert_eq!(
            recommended_next(Phase::RegistryAbsent, M),
            Some(Action::InitRegistry)
        );
        assert_eq!(
            recommended_next(Phase::VaultUnseeded, M),
            Some(Action::Deposit)
        );
    }

    fn seat(idx: u32, leader: Pubkey, seeded: bool) -> VaultSeat {
        VaultSeat {
            idx,
            seq: u64::from(idx) + 1,
            leader,
            seeded,
            stake: accounts::VaultStake::default(),
        }
    }

    #[test]
    fn create_vault_refuses_when_the_leader_already_leads_one() {
        // The one guard against a second vault for a leader: the program does
        // not enforce one-vault-per-leader, so this is it.
        let me = Pubkey::new_unique();
        let stranger = Pubkey::new_unique();
        // A stranger's vault on the market does not block ours.
        assert_eq!(existing_seat(&[seat(0, stranger, true)], &me), None);
        // Ours, seeded or not, does.
        let seats = [seat(0, stranger, true), seat(4, me, false)];
        assert_eq!(existing_seat(&seats, &me), Some(seats[1]));
    }

    #[test]
    fn deposit_seeds_only_the_leaders_single_empty_vault() {
        let me = Pubkey::new_unique();
        let stranger = Pubkey::new_unique();
        assert_eq!(
            deposit_seat(&[seat(0, stranger, false)], &me),
            DepositSeat::NoVault
        );
        assert_eq!(
            deposit_seat(&[seat(0, stranger, false), seat(2, me, false)], &me),
            DepositSeat::Fund(seat(2, me, false))
        );
        // Never on top of a deposit — the retry case.
        assert_eq!(
            deposit_seat(&[seat(2, me, true)], &me),
            DepositSeat::Seeded(seat(2, me, true))
        );
        // Two under one leader is ambiguous, not "pick the first".
        assert_eq!(
            deposit_seat(&[seat(2, me, false), seat(5, me, false)], &me),
            DepositSeat::Ambiguous(2)
        );
    }

    #[test]
    fn over_roster_resumes_skips_and_refuses_when_nothing_was_left() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let log = Logger::new(tx);
        // Every pair already there: the command did nothing, so it refuses.
        let err = over_roster(L, "market", &log, |c| {
            Ok(Outcome::AlreadyThere(c.base.symbol.into()))
        })
        .unwrap_err();
        assert!(format!("{err:#}").contains("refused"), "{err:#}");
        // A partial run: the already-present pairs are skipped, the rest done.
        let mut first = true;
        let summary = over_roster(L, "market", &log, |c| {
            Ok(if std::mem::take(&mut first) {
                Outcome::AlreadyThere(c.base.symbol.into())
            } else {
                Outcome::Done(c.base.symbol.into())
            })
        })
        .unwrap();
        let total = market::roster(L).len();
        assert_eq!(
            summary,
            format!("{} market(s) done, 1 already on-chain", total - 1)
        );
        // The first real error stops the loop: nothing after it runs.
        let mut calls = 0;
        assert!(over_roster(L, "market", &log, |_| {
            calls += 1;
            anyhow::bail!("send failed")
        })
        .is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn every_write_waits_for_a_verified_chain() {
        for a in ALL_ACTIONS {
            let exempt = matches!(a, Action::OpenExplorer | Action::Wipe);
            assert_eq!(a.needs_verified_chain(), !exempt, "{:?}", a.label());
        }
    }

    #[test]
    fn bootstrap_order_is_the_five_ceremony_steps() {
        // Pinned here because `each_bootstrap_step_is_enabled_in_exactly_one_
        // phase` iterates this constant, so it cannot notice a step missing.
        assert_eq!(
            BOOTSTRAP,
            [
                Action::Deploy,
                Action::InitRegistry,
                Action::CreateMarket,
                Action::CreateVault,
                Action::Deposit,
            ]
        );
    }

    #[test]
    fn one_shot_refuses_when_the_account_already_exists() {
        assert_eq!(
            Outcome::Done("made it".into()).one_shot().unwrap(),
            "made it"
        );
        let err = Outcome::AlreadyThere("the registry is already initialized".into())
            .one_shot()
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("refused") && msg.contains("nothing was sent"),
            "{msg}"
        );
    }

    #[test]
    fn mainnet_menu_agrees_with_the_cluster_gate() {
        // Guards the drift between the two hand-written lists: the menu shows
        // exactly the available actions, and nothing available is missing.
        for a in MAINNET_MENU {
            assert!(a.available_on(Cluster::Mainnet), "{:?}", a.label());
        }
        for a in ALL_ACTIONS {
            if a.available_on(Cluster::Mainnet) {
                assert!(MAINNET_MENU.contains(&a), "{:?} missing", a.label());
            }
        }
        assert_eq!(menu_for(Cluster::Mainnet), &MAINNET_MENU);
        assert_eq!(menu_for(Cluster::Localnet), &MENU);
    }

    #[test]
    fn every_mainnet_unavailable_action_states_a_reason() {
        for a in ALL_ACTIONS {
            if !a.available_on(Cluster::Mainnet) {
                assert!(
                    !a.unavailable_reason(Cluster::Mainnet).is_empty(),
                    "{:?}",
                    a.label()
                );
            }
        }
    }

    #[test]
    fn all_actions_covers_the_localnet_menu() {
        // Keeps ALL_ACTIONS honest if a new entry is added to MENU.
        for a in MENU {
            assert!(ALL_ACTIONS.contains(&a), "{:?} missing", a.label());
        }
    }

    #[test]
    fn all_actions_lists_every_variant() {
        // The completeness guard the MENU check above cannot be: MENU holds 12
        // of the 20 variants, so a new shortcut-only action would be absent from
        // both MENU and ALL_ACTIONS and silently escape all four cluster-gate
        // tests — a variant reachable only by a shortcut. PR 2 and PR 3 of this
        // series add exactly that shape.
        //
        // Three assertions together are a complete proof. The match below has
        // one arm and no wildcard, so adding a variant fails to compile here
        // until someone edits this test; the count pins the fixture's size at
        // the enum's size; and the pairwise check rules out duplicates.
        // Twenty distinct variants drawn from a twenty-variant enum is all of
        // them.
        fn is_known(a: Action) -> bool {
            match a {
                Action::Deploy
                | Action::InitRegistry
                | Action::CreateMarket
                | Action::CreateVault
                | Action::Deposit
                | Action::OpenExplorer
                | Action::BootstrapAll
                | Action::ProbeSwap
                | Action::Teardown
                | Action::Wipe
                | Action::LeaderDeposit
                | Action::LeaderWithdraw
                | Action::RotateLeader
                | Action::RepegUp
                | Action::RepegDown
                | Action::WidenSpread
                | Action::TightenSpread
                | Action::ThinFarSide
                | Action::ResetLadder
                | Action::ResetAllLadders => true,
            }
        }
        assert_eq!(
            ALL_ACTIONS.len(),
            20,
            "ALL_ACTIONS must list every Action variant"
        );
        for (i, a) in ALL_ACTIONS.iter().enumerate() {
            assert!(is_known(*a));
            for b in &ALL_ACTIONS[i + 1..] {
                assert_ne!(a, b, "ALL_ACTIONS lists {:?} twice", a.label());
            }
        }
    }

    #[test]
    fn leader_stake_commands_need_a_seeded_book_and_rotation_stays_reserved() {
        let phases = [
            Phase::NoValidator,
            Phase::ProgramAbsent,
            Phase::RegistryAbsent,
            Phase::MarketAbsent,
            Phase::VaultAbsent,
            Phase::VaultUnseeded,
            Phase::Ready,
        ];
        for cluster in [L, M] {
            for phase in phases {
                let ready = phase == Phase::Ready;
                assert_eq!(Action::LeaderDeposit.enabled(phase, cluster), ready);
                assert_eq!(Action::LeaderWithdraw.enabled(phase, cluster), ready);
                // Reserved, not guessed at: no phase on either cluster arms it.
                assert!(!Action::RotateLeader.enabled(phase, cluster));
            }
            assert!(!Action::RotateLeader
                .disabled_reason(Phase::Ready, cluster)
                .is_empty());
        }
        // None of them is a bootstrap step, so none is ever recommended.
        for a in [
            Action::LeaderDeposit,
            Action::LeaderWithdraw,
            Action::RotateLeader,
        ] {
            assert!(!BOOTSTRAP.contains(&a));
        }
    }

    #[test]
    fn recommended_next_follows_the_bootstrap_order() {
        assert_eq!(recommended_next(Phase::NoValidator, L), None);
        assert_eq!(
            recommended_next(Phase::ProgramAbsent, L),
            Some(Action::Deploy)
        );
        assert_eq!(
            recommended_next(Phase::RegistryAbsent, L),
            Some(Action::InitRegistry)
        );
        assert_eq!(
            recommended_next(Phase::MarketAbsent, L),
            Some(Action::CreateMarket)
        );
        assert_eq!(
            recommended_next(Phase::VaultAbsent, L),
            Some(Action::CreateVault)
        );
        assert_eq!(
            recommended_next(Phase::VaultUnseeded, L),
            Some(Action::Deposit)
        );
        assert_eq!(recommended_next(Phase::Ready, L), None);
    }

    #[test]
    fn each_bootstrap_step_is_enabled_in_exactly_one_phase() {
        for cluster in [L, M] {
            for step in BOOTSTRAP {
                let count = PHASES.iter().filter(|p| step.enabled(**p, cluster)).count();
                assert_eq!(count, 1, "{step:?} should be enabled in exactly one phase");
            }
        }
    }

    #[test]
    fn teardown_enabled_once_the_program_is_deployed() {
        assert!(!Action::Teardown.enabled(Phase::NoValidator, L));
        assert!(!Action::Teardown.enabled(Phase::ProgramAbsent, L));
        for p in [
            Phase::RegistryAbsent,
            Phase::MarketAbsent,
            Phase::VaultAbsent,
            Phase::VaultUnseeded,
            Phase::Ready,
        ] {
            assert!(
                Action::Teardown.enabled(p, L),
                "teardown should run in {p:?}"
            );
        }
    }

    #[test]
    fn bootstrap_all_spans_deploy_through_deposit() {
        // "Bootstrap all" self-deploys, so it's enabled from the moment a
        // validator is up (program still absent) until everything exists.
        for p in [
            Phase::ProgramAbsent,
            Phase::RegistryAbsent,
            Phase::MarketAbsent,
            Phase::VaultAbsent,
            Phase::VaultUnseeded,
        ] {
            assert!(
                Action::BootstrapAll.enabled(p, L),
                "bootstrap all should run in {p:?}"
            );
        }
        assert!(!Action::BootstrapAll.enabled(Phase::NoValidator, L));
        assert!(!Action::BootstrapAll.enabled(Phase::Ready, L));
        // Once everything exists, it really is already bootstrapped.
        assert_eq!(
            Action::BootstrapAll.disabled_reason(Phase::Ready, L),
            "already bootstrapped"
        );
        assert_eq!(
            Action::BootstrapAll.disabled_reason(Phase::NoValidator, L),
            "waiting for validator"
        );
    }

    #[test]
    fn eclob_controls_run_only_when_ready() {
        // The reprice / reshape keybinds quote against a live, seeded vault, so
        // they light up in `Ready` alone — and grey out with a vault reason
        // everywhere else (once a validator is up).
        let controls = [
            Action::RepegUp,
            Action::RepegDown,
            Action::WidenSpread,
            Action::TightenSpread,
            Action::ThinFarSide,
            Action::ResetLadder,
            Action::ResetAllLadders,
        ];
        for c in controls {
            for p in PHASES {
                assert_eq!(
                    c.enabled(p, L),
                    p == Phase::Ready,
                    "{c:?} should be enabled only in Ready, not {p:?}"
                );
            }
            assert_eq!(
                c.disabled_reason(Phase::VaultAbsent, L),
                "needs a live, seeded vault"
            );
        }
    }

    #[test]
    fn wipe_always_enabled_and_explorer_needs_a_validator() {
        for p in PHASES {
            assert!(Action::Wipe.enabled(p, L));
            assert_eq!(Action::OpenExplorer.enabled(p, L), p != Phase::NoValidator);
        }
    }
}
