//! Live on-chain state — the brain of the control panel.
//!
//! Every refresh re-derives a [`ChainState`] snapshot purely from what the
//! validator reports (no local "what's done" flag that could drift after a
//! relaunch), and [`ChainState::phase`] collapses it to the [`Phase`] that
//! gates the action menu. Because the snapshot is chain-derived, relaunching
//! the TUI against an already-bootstrapped validator lands in `Ready`, not a
//! reset — the market is discovered by scanning the program's accounts for
//! the `MarketHeader` discriminator, and the fee mint is read back from the
//! registry's stamped default fee config, so nothing depends on mint
//! keypairs held only in a previous session's memory.

use crate::chain;
use dropset_sdk::accounts::{
    fetch_maybe_registry_header, VaultDepositorHeader, MARKET_HEADER_DISCRIMINATOR,
    VAULT_DEPOSITOR_HEADER_DISCRIMINATOR,
};
use dropset_sdk::layout::MarketView as SlabView;
use dropset_sdk::shared::MaybeAccount;
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;

/// The bootstrap progression. Each action is enabled in exactly one phase
/// (plus the always-on ones); the order here is the order they unlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    NoValidator,
    ProgramAbsent,
    RegistryAbsent,
    MarketAbsent,
    VaultAbsent,
    Ready,
}

impl Phase {
    /// A short human label for the status bar.
    pub fn label(self) -> &'static str {
        match self {
            Phase::NoValidator => "No validator",
            Phase::ProgramAbsent => "Program absent",
            Phase::RegistryAbsent => "Registry absent",
            Phase::MarketAbsent => "Market absent",
            Phase::VaultAbsent => "Vault absent",
            Phase::Ready => "Ready",
        }
    }
}

/// Decoded registry view.
#[derive(Clone, Debug)]
pub struct RegistryView {
    pub address: Pubkey,
    pub lamports: u64,
    pub fee_mint: Pubkey,
    pub fee_token_program: Pubkey,
    pub fee_vault: Pubkey,
    pub fee_vault_lamports: u64,
    pub market_count: u32,
}

/// Decoded market view (the single localnet market, if one exists).
#[derive(Clone, Debug)]
pub struct MarketView {
    pub address: Pubkey,
    pub lamports: u64,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
    pub base_treasury_lamports: u64,
    pub quote_treasury_lamports: u64,
    pub active_count: u32,
    /// `(sector_index, leader)` for every live vault — drives teardown.
    pub live_vaults: Vec<(u32, Pubkey)>,
    /// `(sector_index, owner)` for every open `VaultDepositor` on this
    /// market — the first leg of teardown (`force_withdraw_depositor`).
    pub depositors: Vec<(u32, Pubkey)>,
}

/// A full snapshot of localnet state at one refresh.
#[derive(Clone, Debug, Default)]
pub struct ChainState {
    pub validator_up: bool,
    pub slot: Option<u64>,
    pub program_deployed: bool,
    pub registry: Option<RegistryView>,
    pub market: Option<MarketView>,
    pub wallet_lamports: u64,
}

impl ChainState {
    /// Derive the gating [`Phase`] from the snapshot.
    pub fn phase(&self) -> Phase {
        if !self.validator_up {
            return Phase::NoValidator;
        }
        if !self.program_deployed {
            return Phase::ProgramAbsent;
        }
        if self.registry.is_none() {
            return Phase::RegistryAbsent;
        }
        match &self.market {
            None => Phase::MarketAbsent,
            Some(m) if m.active_count == 0 => Phase::VaultAbsent,
            Some(_) => Phase::Ready,
        }
    }
}

/// Refresh the snapshot. Each layer is only queried once the previous one
/// exists, mirroring the phase progression and avoiding RPC calls that
/// would error before the program is deployed.
pub fn poll(client: &RpcClient, wallet: &Pubkey) -> ChainState {
    let slot = client.get_slot().ok();
    let mut state = ChainState {
        validator_up: slot.is_some(),
        slot,
        wallet_lamports: client.get_balance(wallet).unwrap_or(0),
        ..Default::default()
    };
    if !state.validator_up {
        return state;
    }

    // Program account at DROPSET_ID is owned by the loader and executable
    // once deployed.
    state.program_deployed = client
        .get_account(&DROPSET_ID)
        .map(|a| a.executable)
        .unwrap_or(false);
    if !state.program_deployed {
        return state;
    }

    state.registry = read_registry(client);
    if state.registry.is_none() {
        return state;
    }

    state.market = read_market(client);
    state
}

/// Decode the registry via the SDK's typed `fetch_*` path, deriving its
/// stamped fee vault and reading that vault's lamports.
fn read_registry(client: &RpcClient) -> Option<RegistryView> {
    let address = chain::registry_pda();
    let MaybeAccount::Exists(decoded) = fetch_maybe_registry_header(client, &address).ok()? else {
        return None;
    };
    let fee = decoded.data.default_fee_config;
    let fee_vault = chain::associated_token_address(&address, &fee.mint, &fee.token_program);
    let fee_vault_lamports = client.get_balance(&fee_vault).unwrap_or(0);
    Some(RegistryView {
        address,
        lamports: decoded.account.lamports,
        fee_mint: fee.mint,
        fee_token_program: fee.token_program,
        fee_vault,
        fee_vault_lamports,
        market_count: decoded.data.market_count,
    })
}

/// Discover the localnet market by scanning the program's owned accounts
/// for the `MarketHeader` discriminator, then decode its header + active
/// vault list via the slab-layout mirror.
fn read_market(client: &RpcClient) -> Option<MarketView> {
    let accounts = client.get_program_accounts(&DROPSET_ID).ok()?;
    let market_idx = accounts
        .iter()
        .position(|(_, a)| a.data.len() >= 8 && a.data[..8] == MARKET_HEADER_DISCRIMINATOR)?;
    let address = accounts[market_idx].0;
    let account = &accounts[market_idx].1;

    let view = SlabView::load(&account.data).ok()?;
    let header = view.header;
    let base_treasury = Pubkey::new_from_array(header.base_treasury);
    let quote_treasury = Pubkey::new_from_array(header.quote_treasury);
    let live_vaults: Vec<(u32, Pubkey)> = view
        .active_vaults()
        .map(|(idx, v)| (idx, Pubkey::new_from_array(v.leader)))
        .collect();

    // Open VaultDepositor PDAs for this market — discovered in the same
    // program-accounts scan, decoded for their (sector, owner).
    let depositors: Vec<(u32, Pubkey)> = accounts
        .iter()
        .filter(|(_, a)| a.data.len() >= 8 && a.data[..8] == VAULT_DEPOSITOR_HEADER_DISCRIMINATOR)
        .filter_map(|(_, a)| VaultDepositorHeader::from_bytes(&a.data).ok())
        .filter(|h| h.market == address)
        .map(|h| (h.sector_idx, h.owner))
        .collect();

    // Treasury ATAs are SPL-owned, so they aren't in the program-accounts
    // scan — read their lamports directly.
    let balances = client
        .get_multiple_accounts(&[base_treasury, quote_treasury])
        .ok()?;
    let lamports = |i: usize| {
        balances
            .get(i)
            .and_then(|o| o.as_ref())
            .map_or(0, |a| a.lamports)
    };

    Some(MarketView {
        address,
        lamports: account.lamports,
        base_mint: Pubkey::new_from_array(header.base_mint),
        quote_mint: Pubkey::new_from_array(header.quote_mint),
        base_treasury,
        quote_treasury,
        base_treasury_lamports: lamports(0),
        quote_treasury_lamports: lamports(1),
        active_count: header.active_count.get(),
        live_vaults,
        depositors,
    })
}
