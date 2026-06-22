//! The action menu: what each entry does, when it is enabled, and how it
//! dispatches to a background job.
//!
//! Availability is a pure function of the derived [`Phase`] — the panel is
//! always truthful about what is possible right now, and greys out the rest
//! with a one-line reason. Bootstrapping is a sequence of discrete gated
//! steps (deploy → init → create-market → create-vault) so each account and
//! its rent can be watched appearing one at a time; "Bootstrap all" chains
//! the on-chain steps for convenience.

use crate::accounts::{self, ChainState, Phase};
use crate::chain;
use crate::deploy;
use crate::explorer;
use crate::job::{self, JobEvent, Logger};
use anyhow::{Context, Result};
use solana_keypair::Keypair;
use solana_native_token::LAMPORTS_PER_SOL;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::PathBuf;
use std::sync::mpsc::Sender;

/// A menu action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Deploy,
    InitRegistry,
    CreateMarket,
    CreateVault,
    OpenExplorer,
    BootstrapAll,
    Teardown,
    Wipe,
}

/// The menu in display order. Indices map to the `1..=8` number keys.
pub const MENU: [Action; 8] = [
    Action::Deploy,
    Action::InitRegistry,
    Action::CreateMarket,
    Action::CreateVault,
    Action::OpenExplorer,
    Action::BootstrapAll,
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
            Action::Teardown => "Teardown & reclaim",
            Action::Wipe => "Wipe localnet",
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
            Action::BootstrapAll => matches!(
                phase,
                Phase::RegistryAbsent | Phase::MarketAbsent | Phase::VaultAbsent
            ),
            Action::Teardown => matches!(phase, Phase::VaultAbsent | Phase::Ready),
            Action::Wipe => true,
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
            Action::Teardown => "nothing to tear down yet",
            Action::OpenExplorer | Action::Wipe => "",
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
}

impl JobContext {
    fn wallet(&self) -> Keypair {
        self.wallet.insecure_clone()
    }
}

/// Spawn the background job for `action`. [`Action::Wipe`] is handled by the
/// event loop instead (it mutates the owned validator), so it is a no-op
/// here.
pub fn dispatch(action: Action, ctx: &JobContext, state: &ChainState, tx: Sender<JobEvent>) {
    let rpc_url = ctx.rpc_url.clone();
    let repo_root = ctx.repo_root.clone();
    let wallet_path = ctx.wallet_path.clone();
    let wallet = ctx.wallet();

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
            job::spawn(tx, "Create market", move |log| {
                let client = chain::rpc(&rpc_url);
                do_create_market(&client, &wallet, log)
            });
        }
        Action::CreateVault => {
            job::spawn(tx, "Create vault", move |log| {
                let client = chain::rpc(&rpc_url);
                do_create_vault(&client, &wallet, log)
            });
        }
        Action::BootstrapAll => {
            job::spawn(tx, "Bootstrap all", move |log| {
                let client = chain::rpc(&rpc_url);
                do_init(&client, &wallet, log)?;
                do_create_market(&client, &wallet, log)?;
                do_create_vault(&client, &wallet, log)?;
                Ok("Bootstrap complete".into())
            });
        }
        Action::Teardown => {
            job::spawn(tx, "Teardown", move |log| {
                let client = chain::rpc(&rpc_url);
                do_teardown(&client, &wallet, log)
            });
        }
        Action::OpenExplorer => {
            let urls = explorer_targets(state);
            job::spawn(tx, "Open explorer", move |log| {
                for (label, addr) in &urls {
                    log.log(format!("Opening {label} {addr}"));
                    explorer::open_account(&rpc_url, addr)
                        .with_context(|| format!("open {label}"))?;
                }
                Ok(format!("Opened {} account(s) in explorer", urls.len()))
            });
        }
        // Wipe is handled by the event loop (owns the validator).
        Action::Wipe => {}
    }
}

/// `(label, address)` pairs to open in the explorer for the current state —
/// every account that currently exists.
fn explorer_targets(state: &ChainState) -> Vec<(&'static str, Pubkey)> {
    let mut targets = vec![("program", dropset_sdk::DROPSET_ID)];
    if let Some(reg) = &state.registry {
        targets.push(("registry", reg.address));
        targets.push(("registry fee vault", reg.fee_vault));
    }
    if let Some(mkt) = &state.market {
        targets.push(("market", mkt.address));
        targets.push(("base treasury", mkt.base_treasury));
        targets.push(("quote treasury", mkt.quote_treasury));
    }
    targets
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
    let sig = chain::send(client, wallet, &[wallet], &[ix, topup]).context("send init")?;
    log.log(format!("init: {sig}"));
    log.accounts_changed();
    Ok("Registry initialized".into())
}

/// Create the market: mint mock base/quote mints, then `create_market`
/// charged (and waived, admin) against the registry's stamped fee mint.
fn do_create_market(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), log);
    let registry = accounts::poll(client, &wallet.pubkey())
        .registry
        .context("registry not found — init first")?;
    log.log("Creating mock base + quote mints…");
    let base_mint = chain::create_spl_mint(client, wallet).context("create base mint")?;
    let quote_mint = chain::create_spl_mint(client, wallet).context("create quote mint")?;
    log.log(format!("base: {base_mint}"));
    log.log(format!("quote: {quote_mint}"));
    let ix = chain::build_create_market_ix(
        &wallet.pubkey(),
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
    let sig = chain::send(client, wallet, &[wallet], &[ix, topup]).context("send create_market")?;
    log.log(format!("create_market: {sig}"));
    log.accounts_changed();
    Ok("Market created".into())
}

/// Create the leader vault on the market via the admin path.
fn do_create_vault(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    log: &Logger,
) -> Result<String> {
    ensure_funded(client, &wallet.pubkey(), log);
    let state = accounts::poll(client, &wallet.pubkey());
    let registry = state.registry.context("registry not found")?;
    let market = state
        .market
        .context("market not found — create market first")?;
    let ix = chain::build_create_vault_ix(
        &wallet.pubkey(),
        &market.address,
        &registry.fee_mint,
        &registry.fee_token_program,
    );
    // Trailing rent top-up for the market PDA, which create_vault grows —
    // see RENT_TOPUP_LAMPORTS.
    let topup = chain::system_transfer_ix(
        &wallet.pubkey(),
        &market.address,
        chain::RENT_TOPUP_LAMPORTS,
    );
    let sig = chain::send(client, wallet, &[wallet], &[ix, topup]).context("send create_vault")?;
    log.log(format!("create_vault: {sig}"));
    log.accounts_changed();
    Ok("Vault created".into())
}

/// Admin teardown in the verified order, refunding all rent to the wallet:
/// per-leader `force_withdraw_leader` → per-leg `close_market_treasury` →
/// `close_market` → `close_registry_fee_vault` → `close_registry`. (The
/// per-depositor `force_withdraw_depositor` step is a no-op here — the TUI
/// never opens outside depositors.) Logs the reclaimed-rent lamports delta.
fn do_teardown(
    client: &solana_client::rpc_client::RpcClient,
    wallet: &Keypair,
    log: &Logger,
) -> Result<String> {
    let admin = wallet.pubkey();
    let state = accounts::poll(client, &admin);
    let registry = state.registry.context("registry not found")?;
    let market = state.market.context("market not found")?;
    let before = client.get_balance(&admin).unwrap_or(0);

    for (sector, owner) in &market.depositors {
        log.log(format!(
            "force_withdraw_depositor sector {sector} owner {owner}"
        ));
        let ix = chain::build_force_withdraw_depositor_ix(
            &admin,
            &market.address,
            &market.base_mint,
            &market.quote_mint,
            &market.base_treasury,
            &market.quote_treasury,
            *sector,
            owner,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("force_withdraw_depositor sector {sector}"))?;
        log.log(format!("  {sig}"));
        log.accounts_changed();
    }

    if market.live_vaults.is_empty() {
        log.log("No live vaults to drain.");
    }
    for (sector, leader) in &market.live_vaults {
        log.log(format!(
            "force_withdraw_leader sector {sector} leader {leader}"
        ));
        let ix = chain::build_force_withdraw_leader_ix(
            &admin,
            &market.address,
            &market.base_mint,
            &market.quote_mint,
            &market.base_treasury,
            &market.quote_treasury,
            *sector,
            leader,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("force_withdraw_leader sector {sector}"))?;
        log.log(format!("  {sig}"));
        log.accounts_changed();
    }

    for (leg, mint, treasury) in [
        ("base", market.base_mint, market.base_treasury),
        ("quote", market.quote_mint, market.quote_treasury),
    ] {
        log.log(format!("close_market_treasury ({leg})"));
        let ix = chain::build_close_market_treasury_ix(
            &admin,
            &market.address,
            &mint,
            &treasury,
            &admin,
        );
        let sig = chain::send(client, wallet, &[wallet], &[ix])
            .with_context(|| format!("close_market_treasury {leg}"))?;
        log.log(format!("  {sig}"));
    }

    log.log("close_market");
    let ix = chain::build_close_market_ix(
        &admin,
        &market.address,
        &market.base_treasury,
        &market.quote_treasury,
        &admin,
    );
    let sig = chain::send(client, wallet, &[wallet], &[ix]).context("close_market")?;
    log.log(format!("  {sig}"));
    log.accounts_changed();

    log.log("close_registry_fee_vault");
    let ix = chain::build_close_registry_fee_vault_ix(
        &admin,
        &registry.fee_mint,
        &registry.fee_token_program,
        &admin,
    );
    let sig = chain::send(client, wallet, &[wallet], &[ix]).context("close_registry_fee_vault")?;
    log.log(format!("  {sig}"));

    log.log("close_registry");
    let ix = chain::build_close_registry_ix(&admin, &admin);
    let sig = chain::send(client, wallet, &[wallet], &[ix]).context("close_registry")?;
    log.log(format!("  {sig}"));
    log.accounts_changed();

    let after = client.get_balance(&admin).unwrap_or(0);
    let reclaimed = after.saturating_sub(before);
    Ok(format!(
        "Teardown complete — reclaimed {:.6} SOL in rent",
        reclaimed as f64 / LAMPORTS_PER_SOL as f64
    ))
}
