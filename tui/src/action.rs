//! The action menu: what each entry does, when it is enabled, and how it
//! dispatches to a background job.
//!
//! Availability is a pure function of the derived [`Phase`] — the panel is
//! always truthful about what is possible right now, and greys out the rest
//! with a one-line reason. Bootstrapping is a sequence of discrete gated
//! steps (deploy → init → create-market → create-vault) so each account and
//! its rent can be watched appearing one at a time; "Bootstrap all" chains
//! the whole sequence — deploying the program first when it isn't yet
//! on-chain — for convenience.

use crate::accounts::{self, ChainState, Phase};
use crate::chain;
use crate::deploy;
use crate::explorer;
use crate::job::{self, JobEvent, Logger};
use crate::market::{self, PairConfig};
use crate::teardown;
use anyhow::{Context, Result};
use dropset_sdk::matching::SwapSide;
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{set_liquidity_profile_ix, set_reference_price_ix};
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
/// The first block is the bootstrap-lifecycle menu (the numbered `1..=9`
/// entries in [`MENU`]). The trailing block are the eCLOB demo controls —
/// market-scoped keybinds, *not* menu entries — that fire the two quoting
/// instructions independently on the selected market to show the "reprice vs
/// reshape" distinction live: [`Action::RepegUp`] / [`Action::RepegDown`] move
/// the whole ladder (`set_reference_price`), while the reshape actions change
/// the ladder's shape at a fixed peg (`set_liquidity_profile`).
///
/// Both compete with a running maker bot, which re-quotes every tick and
/// overwrites a manual nudge within ~1s — stop the market's bot (`s` / `x`)
/// for a stable on-stage demo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Deploy,
    InitRegistry,
    CreateMarket,
    CreateVault,
    OpenExplorer,
    BootstrapAll,
    ProbeSwap,
    Teardown,
    Wipe,
    // eCLOB demo controls — keybinds, not menu entries.
    RepegUp,
    RepegDown,
    WidenSpread,
    TightenSpread,
    ThinFarSide,
    ResetLadder,
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
const NEVER_EXPIRES: u32 = u32::MAX;

/// The menu in display order. Indices map to the `1..=9` number keys.
pub const MENU: [Action; 9] = [
    Action::Deploy,
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
    Action::OpenExplorer,
    Action::BootstrapAll,
    Action::ProbeSwap,
    Action::Teardown,
    Action::Wipe,
];

/// The ordered bootstrap steps, used to pick the recommended next step.
const BOOTSTRAP: [Action; 4] = [
    Action::Deploy,
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
];

impl Action {
    /// Menu label.
    pub fn label(self) -> &'static str {
        match self {
            Action::Deploy => "Deploy program",
            Action::InitRegistry => "Init registry",
            Action::CreateMarket => "Create market",
            Action::CreateVault => "Create vault",
            Action::OpenExplorer => "Open explorer",
            Action::BootstrapAll => "Bootstrap all",
            Action::ProbeSwap => "Probe swap (CU)",
            Action::Teardown => "Teardown & reclaim",
            Action::Wipe => "Wipe localnet",
            Action::RepegUp => "Re-peg +5 bps",
            Action::RepegDown => "Re-peg -5 bps",
            Action::WidenSpread => "Widen spread",
            Action::TightenSpread => "Tighten spread",
            Action::ThinFarSide => "Thin far side",
            Action::ResetLadder => "Reset ladder",
        }
    }

    /// Whether the action can run in `phase`.
    pub fn enabled(self, phase: Phase) -> bool {
        match self {
            Action::Deploy => phase == Phase::ProgramAbsent,
            Action::InitRegistry => phase == Phase::RegistryAbsent,
            Action::CreateMarket => phase == Phase::MarketAbsent,
            Action::CreateVault => phase == Phase::VaultAbsent,
            Action::OpenExplorer => phase != Phase::NoValidator,
            // Self-deploys when the program is absent, so it's available the
            // moment a validator is up and runs until everything exists.
            Action::BootstrapAll => matches!(
                phase,
                Phase::ProgramAbsent
                    | Phase::RegistryAbsent
                    | Phase::MarketAbsent
                    | Phase::VaultAbsent
            ),
            // A take needs a live, seeded vault to match against.
            Action::ProbeSwap => phase == Phase::Ready,
            // Reclaim whatever exists from the program onward — program
            // rent, the registry + fee vault, and the market if present.
            Action::Teardown => matches!(
                phase,
                Phase::RegistryAbsent | Phase::MarketAbsent | Phase::VaultAbsent | Phase::Ready
            ),
            Action::Wipe => true,
            // The demo controls quote against a live vault.
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder => phase == Phase::Ready,
        }
    }

    /// One-line reason the action is greyed out in `phase` (only meaningful
    /// when [`Action::enabled`] is false).
    pub fn disabled_reason(self, phase: Phase) -> &'static str {
        if phase == Phase::NoValidator {
            return "waiting for validator";
        }
        match self {
            Action::Deploy => "program already deployed",
            Action::InitRegistry if phase == Phase::ProgramAbsent => "deploy the program first",
            Action::InitRegistry => "registry already initialized",
            Action::CreateMarket if below(phase, Phase::MarketAbsent) => {
                "initialize the registry first"
            }
            Action::CreateMarket => "market already exists",
            Action::CreateVault if below(phase, Phase::VaultAbsent) => "create the market first",
            Action::CreateVault => "vault already exists",
            Action::BootstrapAll => "already bootstrapped",
            Action::ProbeSwap => "needs a live, seeded vault",
            Action::Teardown => "deploy the program first",
            Action::OpenExplorer | Action::Wipe => "",
            Action::RepegUp
            | Action::RepegDown
            | Action::WidenSpread
            | Action::TightenSpread
            | Action::ThinFarSide
            | Action::ResetLadder => "needs a live, seeded vault",
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
            Phase::Ready => 5,
        }
    }
    rank(a) < rank(b)
}

/// The recommended next bootstrap step in `phase` — the first enabled one.
pub fn recommended_next(phase: Phase) -> Option<Action> {
    BOOTSTRAP.into_iter().find(|a| a.enabled(phase))
}

/// Owned context a background job needs. Cloned per dispatch so the job
/// thread owns everything it touches.
pub struct JobContext {
    pub rpc_url: String,
    pub repo_root: PathBuf,
    pub wallet_path: String,
    pub wallet: Keypair,
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
                do_init(&client, &wallet, log)
            });
        }
        Action::CreateMarket => {
            job::spawn(tx, "Create markets", move |log| {
                let client = chain::rpc(&rpc_url);
                for config in market::PAIRS {
                    do_create_market(&client, &wallet, &repo_root, config, log)?;
                }
                Ok(format!("Created {} markets", market::PAIRS.len()))
            });
        }
        Action::CreateVault => {
            job::spawn(tx, "Create vaults", move |log| {
                let client = chain::rpc(&rpc_url);
                for config in market::PAIRS {
                    do_create_vault(&client, &wallet, &repo_root, config, log)?;
                }
                Ok(format!("Created {} vaults", market::PAIRS.len()))
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
                do_init(&client, &wallet, log)?;
                // Bring up every demo market: its mints, market PDA, and a
                // seeded, quotable vault.
                for config in market::PAIRS {
                    log.log(format!("— {} —", config.base.symbol));
                    do_create_market(&client, &wallet, &repo_root, config, log)?;
                    do_create_vault(&client, &wallet, &repo_root, config, log)?;
                }
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
                if !explorer::docker_available() {
                    log.log("Docker not found — opening the hosted explorer instead.");
                    log.log(
                        "Note: explorer.solana.com can't reach the localnet in Brave/Safari; \
                         install Docker for the local explorer, or open these links in \
                         Chrome/Firefox.",
                    );
                    open_targets(log, &targets, |addr| {
                        explorer::hosted_account_url(addr, &rpc_url)
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
                open_targets(log, &targets, |addr| explorer::account_url(addr, &rpc_url))?;
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
                    log,
                )
            });
        }
        // Wipe is handled by the event loop (owns the validator).
        Action::Wipe => {}
    }
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
    let ratio = current.quote_for_base(PRICE_SCALE) as f64 / PRICE_SCALE as f64;
    let bumped = ratio * (1.0 + bps / 10_000.0);
    let price =
        Price::from_value(bumped).with_context(|| format!("re-peg to atoms-ratio {bumped}"))?;
    let slot = client.get_slot().context("current slot")?;
    let ix = set_reference_price_ix(leader.pubkey(), market, vault_idx, price, slot);
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
    let human = market::config_for(repo_root, &base_mint)
        .map(|c| bumped * 10f64.powi(c.base.decimals as i32 - c.quote.decimals as i32));
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
    log: &Logger,
) -> Result<String> {
    let (market, base_mint, vault_idx) = eclob_target(market, base_mint, vault_idx)?;
    let config =
        market::config_for(repo_root, &base_mint).context("market not in the bootstrap roster")?;
    let leader = market::leader(repo_root, config)?;
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
            let bids = market::ladder_at_spread_bps(spread_bps);
            let mut asks = bids;
            for (_, size_bps) in &mut asks {
                *size_bps = (f64::from(*size_bps) * THIN_DEPTH_SCALE).round() as u16;
            }
            (
                market::ladder_profile_bytes_asym(&bids, &asks, NEVER_EXPIRES),
                "Thinned the far (ask) side — offer depth shrinks, peg unchanged".to_string(),
            )
        }
        Action::ResetLadder => (
            market::ladder_profile_bytes(
                &market::ladder_at_spread_bps(market::DEFAULT_SPREAD_BPS),
                config.expiry_offset,
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

/// Airdrop a working balance to the wallet if it is running low — admin
/// paths waive fees, but mint creation and tx fees still cost lamports.
fn ensure_funded(client: &solana_client::rpc_client::RpcClient, wallet: &Pubkey, log: &Logger) {
    let balance = client.get_balance(wallet).unwrap_or(0);
    if balance < LAMPORTS_PER_SOL {
        log.log("Airdropping working balance to the wallet…");
        if let Err(e) = chain::airdrop(client, wallet, 100 * LAMPORTS_PER_SOL) {
            log.log(format!("airdrop warning: {e:#}"));
        }
    }
}

/// Create the registry: mint a mock fee mint, then send `init` (genesis
/// admin = wallet, which must equal the program's upgrade authority).
fn do_init(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), log);
    log.log("Creating mock fee mint…");
    let fee_mint = chain::create_spl_mint(client, wallet).context("create fee mint")?;
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
    Ok("Registry initialized".into())
}

/// Create the market: mint `config`'s fixed base/quote pair, then
/// `create_market` charged (and waived, admin) against the registry's
/// stamped fee mint.
fn do_create_market(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    config: &PairConfig,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), log);
    let registry = accounts::poll(client, &wallet.pubkey(), None, 0)
        .registry
        .context("registry not found — init first")?;
    let (base_mint, quote_mint) =
        market::create_pair_mints(client, wallet, repo_root, config, log)?;
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
    Ok("Market created".into())
}

/// Create the leader vault on the market via the admin path, then bring it
/// up live — set `config`'s reference price + quote ladder and seed it with
/// the leader's opening deposit (see [`market::seed_vault`]).
fn do_create_vault(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    config: &PairConfig,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), log);
    let state = accounts::poll(client, &wallet.pubkey(), None, 0);
    let registry = state.registry.context("registry not found")?;
    // Address this config's own market by its PDA — the bootstrap brings up
    // many markets, so the first-found one in `ChainState` isn't necessarily
    // this pair's.
    let (base_mint, quote_mint) = market::pair_mints(repo_root, config)?;
    let market = accounts::read_market_at(client, chain::market_pda(&base_mint, &quote_mint))
        .context("market not found — create market first")?;
    // The vault is opened for `config`'s leader (a distinct role key, not
    // the admin), so admin teardown's force_withdraw_leader doesn't alias
    // the admin signer; the fee source likewise must differ from the payer.
    // The leader also quotes and seeds the vault, so unlike before it must
    // be a real signer with a known key — not a throwaway pubkey.
    let leader = market::leader(repo_root, config)?;
    let fee_source = Keypair::new().pubkey();
    log.log(format!("vault leader: {}", leader.pubkey()));
    let ix = chain::build_create_vault_ix(
        &wallet.pubkey(),
        &fee_source,
        &market.address,
        &registry.fee_mint,
        &registry.fee_token_program,
        &leader.pubkey(),
    );
    // Trailing rent top-up for the market PDA, which create_vault grows —
    // see RENT_TOPUP_LAMPORTS.
    let topup = chain::system_transfer_ix(
        &wallet.pubkey(),
        &market.address,
        chain::RENT_TOPUP_LAMPORTS,
    );
    chain::send_logged(client, wallet, &[wallet], &[ix, topup], "create_vault", log)
        .context("send create_vault")?;
    log.accounts_changed();
    // Bring the vault up quotable + seeded.
    market::seed_vault(client, wallet, &leader, config, &market, log)?;
    log.accounts_changed();
    Ok("Vault created, quoting, and seeded".into())
}

/// Whole quote units a swap probe spends by default (e.g. 10 USDC), scaled by
/// the quote mint's decimals at send time. The TUI seeds its editable swap
/// amount with this; the taker overrides it via the amount input (`a`).
pub const DEFAULT_PROBE_QUOTE_UNITS: u64 = 10;

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
    ensure_funded(client, &wallet.pubkey(), log);
    // Swap against the selected market when one is set, else the first the scan
    // finds — matching the book the operator is looking at.
    let market = match target {
        Some(address) => accounts::read_market_at(client, address),
        None => accounts::poll(client, &wallet.pubkey(), None, 0)
            .markets
            .into_iter()
            .next(),
    }
    .context("no market — bootstrap first")?;
    if market.active_count == 0 {
        anyhow::bail!("no live vault to swap against — create the vault first");
    }

    // The taker is FFFF — fund it with SOL so it pays its own fee, and give it
    // both ATAs (admin is the mint authority): one holds the leg it pays, the
    // other receives the leg it gets. Which is which flips with the side.
    let taker = market::taker(repo_root)?;
    let taker_pk = taker.pubkey();
    ensure_funded(client, &taker_pk, log);
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
        SwapSide::Sell => (market.base_mint, base_ata, market.base_decimals, Price::ZERO),
    };
    let notional = 10u64
        .pow(input_decimals as u32)
        .saturating_mul(units);
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

    const PHASES: [Phase; 6] = [
        Phase::NoValidator,
        Phase::ProgramAbsent,
        Phase::RegistryAbsent,
        Phase::MarketAbsent,
        Phase::VaultAbsent,
        Phase::Ready,
    ];

    #[test]
    fn recommended_next_follows_the_bootstrap_order() {
        assert_eq!(recommended_next(Phase::NoValidator), None);
        assert_eq!(recommended_next(Phase::ProgramAbsent), Some(Action::Deploy));
        assert_eq!(
            recommended_next(Phase::RegistryAbsent),
            Some(Action::InitRegistry)
        );
        assert_eq!(
            recommended_next(Phase::MarketAbsent),
            Some(Action::CreateMarket)
        );
        assert_eq!(
            recommended_next(Phase::VaultAbsent),
            Some(Action::CreateVault)
        );
        assert_eq!(recommended_next(Phase::Ready), None);
    }

    #[test]
    fn each_bootstrap_step_is_enabled_in_exactly_one_phase() {
        for step in [
            Action::Deploy,
            Action::InitRegistry,
            Action::CreateMarket,
            Action::CreateVault,
        ] {
            let count = PHASES.iter().filter(|p| step.enabled(**p)).count();
            assert_eq!(count, 1, "{step:?} should be enabled in exactly one phase");
        }
    }

    #[test]
    fn teardown_enabled_once_the_program_is_deployed() {
        assert!(!Action::Teardown.enabled(Phase::NoValidator));
        assert!(!Action::Teardown.enabled(Phase::ProgramAbsent));
        for p in [
            Phase::RegistryAbsent,
            Phase::MarketAbsent,
            Phase::VaultAbsent,
            Phase::Ready,
        ] {
            assert!(Action::Teardown.enabled(p), "teardown should run in {p:?}");
        }
    }

    #[test]
    fn bootstrap_all_spans_deploy_through_vault() {
        // "Bootstrap all" self-deploys, so it's enabled from the moment a
        // validator is up (program still absent) until everything exists.
        for p in [
            Phase::ProgramAbsent,
            Phase::RegistryAbsent,
            Phase::MarketAbsent,
            Phase::VaultAbsent,
        ] {
            assert!(
                Action::BootstrapAll.enabled(p),
                "bootstrap all should run in {p:?}"
            );
        }
        assert!(!Action::BootstrapAll.enabled(Phase::NoValidator));
        assert!(!Action::BootstrapAll.enabled(Phase::Ready));
        // Once everything exists, it really is already bootstrapped.
        assert_eq!(
            Action::BootstrapAll.disabled_reason(Phase::Ready),
            "already bootstrapped"
        );
        assert_eq!(
            Action::BootstrapAll.disabled_reason(Phase::NoValidator),
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
        ];
        for c in controls {
            for p in PHASES {
                assert_eq!(
                    c.enabled(p),
                    p == Phase::Ready,
                    "{c:?} should be enabled only in Ready, not {p:?}"
                );
            }
            assert_eq!(
                c.disabled_reason(Phase::VaultAbsent),
                "needs a live, seeded vault"
            );
        }
    }

    #[test]
    fn wipe_always_enabled_and_explorer_needs_a_validator() {
        for p in PHASES {
            assert!(Action::Wipe.enabled(p));
            assert_eq!(Action::OpenExplorer.enabled(p), p != Phase::NoValidator);
        }
    }
}
