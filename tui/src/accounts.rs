//! Live on-chain state — the brain of the control panel.
//!
//! Every refresh re-derives a [`ChainState`] snapshot purely from what the
//! validator reports (no local "what's done" flag that could drift after a
//! relaunch), and [`ChainState::phase`] collapses it to the [`Phase`] that
//! gates the action menu. Because the snapshot is chain-derived, relaunching
//! the TUI against an already-bootstrapped validator lands in `Ready`, not a
//! reset — every market is discovered by scanning the program's accounts for
//! the `MarketHeader` discriminator, and the fee mint is read back from the
//! registry's stamped default fee config, so nothing depends on mint
//! keypairs held only in a previous session's memory.

// cspell:word keypairs

use crate::chain;
use dropset_sdk::accounts::{
    fetch_maybe_registry_header, VaultDepositorHeader, MARKET_HEADER_DISCRIMINATOR,
    VAULT_DEPOSITOR_HEADER_DISCRIMINATOR,
};
use dropset_sdk::layout::MarketView as SlabView;
use dropset_sdk::matching::{resting_levels, BookLevel, SwapSide};
use dropset_sdk::price::Price;
use dropset_sdk::shared::MaybeAccount;
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_pubkey::Pubkey;

/// A maker `quote_slot` this many slots behind the poll's head slot still
/// counts as [`Liveness::Live`]. The maker stamps a reference price at least
/// every `ref_heartbeat` (30 s — `bots/maker-bot` `StrategyConfig`); at
/// localnet's ~400 ms/slot that is ~75 slots, so ~3 heartbeats' worth of slots
/// absorbs a late or missed heartbeat and poll jitter without flapping to
/// stale, while a stopped bot still crosses into stale within ~90 s.
const MAKER_LIVE_WITHIN_SLOTS: u64 = 225;

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

/// Decoded view of one localnet market — the demo brings up several, and
/// [`ChainState::markets`] holds them all.
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
    /// The `reference_price.quote_slot` of the first live vault — the one whose
    /// leader the accounts pane shows as the MM bot. Drives the leader's
    /// liveness (freshness against the poll's head slot). `None` when the market
    /// has no live vault; `Some(0)` for a vault that has never quoted (reads as
    /// maximally stale).
    pub leader_quote_slot: Option<u32>,
    /// The first live vault's stamped reference price, in human quote-per-base
    /// units — the fair value the maker pegs to, shown per market in the markets
    /// pane. `None` when the market has no live vault or its reference is unset /
    /// sentinel (zero / infinity), so the pane can show a placeholder.
    pub reference_price: Option<f64>,
    /// `(sector_index, owner)` for every open `VaultDepositor` on this
    /// market — the first leg of teardown (`force_withdraw_depositor`).
    pub depositors: Vec<(u32, Pubkey)>,
    /// Base / quote mint decimals — for scaling book prices and sizes to
    /// human units in the order-book pane.
    pub base_decimals: u8,
    pub quote_decimals: u8,
    /// The reconstructed resting book at the poll's slot, in cross-vault
    /// price-time priority (best first). `asks` ascend in price, `bids`
    /// descend; sizes are base atoms (see [`resting_levels`]).
    pub asks: Vec<BookLevel>,
    pub bids: Vec<BookLevel>,
}

/// How live a bot participant looks, as the accounts pane can observe it
/// without any bot-side heartbeat account. For the maker it is derived purely
/// from how recently it stamped a reference price on chain — its vault's
/// `quote_slot` versus the poll's head slot — so a bot that is quoting reads
/// [`Liveness::Live`] and one that has gone quiet reads [`Liveness::Stale`],
/// independent of who launched it. The taker leaves no such on-chain footprint
/// (its flow is deliberately quiet between bursts, so activity would flap), so
/// its liveness is process-based: the TUI reads it [`Liveness::Live`] exactly
/// while it is running that market's taker child (set after the poll, in
/// `App::maybe_refresh`). A participant with no signal is [`Liveness::Unknown`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Liveness {
    /// Quoting: stamped a reference price within the freshness window.
    Live,
    /// Present but its last quote has aged past the window — booting, wedged,
    /// or stopped.
    Stale,
    /// No liveness signal is observable for this participant.
    #[default]
    Unknown,
}

/// A market participant's wallet token holdings — the swapper's or the
/// vault leader's (the MM bot's) base/quote ATA balances, in atoms. Lets the
/// accounts pane surface who is trading the market and the inventory in their
/// own wallets (distinct from the vault's, which the treasuries show).
#[derive(Clone, Debug)]
pub struct ParticipantView {
    pub address: Pubkey,
    pub base_tokens: u64,
    pub quote_tokens: u64,
    /// The bot's observed liveness — for the leader (the MM bot), derived from
    /// its vault's quote freshness; [`Liveness::Unknown`] for a participant with
    /// no observable signal.
    pub liveness: Liveness,
}

/// A full snapshot of localnet state at one refresh.
#[derive(Clone, Debug, Default)]
pub struct ChainState {
    pub validator_up: bool,
    pub slot: Option<u64>,
    pub program_deployed: bool,
    pub registry: Option<RegistryView>,
    /// Every localnet market the program-accounts scan discovered, in scan
    /// order — the multi-market demo brings up one per FX pair. The TUI renders
    /// the selected one and shows all of them in the markets list.
    pub markets: Vec<MarketView>,
    pub wallet_lamports: u64,
    /// The vault leader (the MM bot) of the *selected* market's first live
    /// vault, with its wallet token holdings — `None` until a live vault exists.
    pub leader: Option<ParticipantView>,
    /// The swapper / taker (`FFFF`), with its wallet token holdings for the
    /// *selected* market — `None` until a market exists (and the key resolves).
    pub swapper: Option<ParticipantView>,
}

impl ChainState {
    /// Derive the gating [`Phase`] from the snapshot. The bootstrap actions
    /// bring up every demo market together, so the phase is an aggregate:
    /// `Ready` only once every discovered market has a live vault.
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
        if self.markets.is_empty() {
            return Phase::MarketAbsent;
        }
        if self.markets.iter().all(|m| m.active_count > 0) {
            Phase::Ready
        } else {
            Phase::VaultAbsent
        }
    }

    /// The market at `selected`, clamped so an out-of-range index (markets
    /// disappeared on a wipe) still yields the first one rather than `None`
    /// when any market exists.
    pub fn selected_market(&self, selected: usize) -> Option<&MarketView> {
        if self.markets.is_empty() {
            return None;
        }
        self.markets.get(selected).or_else(|| self.markets.first())
    }
}

/// Refresh the snapshot. Each layer is only queried once the previous one
/// exists, mirroring the phase progression and avoiding RPC calls that
/// would error before the program is deployed. `swapper` is the taker role
/// key (`FFFF`); when supplied, its wallet token holdings are read for the
/// accounts pane (`None` skips that — the bootstrap jobs that poll only for
/// registry pass `None`). `selected` picks which discovered market's
/// participants (leader / swapper holdings) to read for the accounts pane.
pub fn poll(
    client: &RpcClient,
    wallet: &Pubkey,
    swapper: Option<&Pubkey>,
    selected: usize,
) -> ChainState {
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

    state.markets = read_markets(client, slot.unwrap_or(0).min(u32::MAX as u64) as u32, None);
    // Participants are read for the selected market only — the accounts pane
    // shows one market at a time, so there is no need to fetch holdings for the
    // whole roster each poll. Cloned so the read borrows nothing of `state`
    // while its `leader` / `swapper` fields are written.
    if let Some(market) = state.selected_market(selected).cloned() {
        // The MM bot is the leader of the market's first live vault; the
        // swapper is the supplied taker key. Read each one's wallet holdings.
        // The leader's liveness is derived here from its vault's quote freshness;
        // the swapper stays `Unknown` — the caller (`App::maybe_refresh`) raises
        // it to `Live` when the TUI is running that market's taker child.
        state.leader = market.live_vaults.first().map(|(_, leader)| {
            let mut view = read_participant(client, leader, &market);
            view.liveness = maker_liveness(slot, market.leader_quote_slot);
            view
        });
        state.swapper = swapper.map(|pk| read_participant(client, pk, &market));
    }
    state
}

/// Classify the maker's liveness from its last quote slot against the poll's
/// head slot: quoting within [`MAKER_LIVE_WITHIN_SLOTS`] is [`Liveness::Live`],
/// anything older (including a vault that has never quoted, `quote_slot == 0`)
/// is [`Liveness::Stale`]. Without a head slot (validator down) there is nothing
/// to compare against, so the result is [`Liveness::Unknown`].
fn maker_liveness(head_slot: Option<u64>, quote_slot: Option<u32>) -> Liveness {
    match (head_slot, quote_slot) {
        (Some(head), Some(quoted)) => {
            if head.saturating_sub(quoted as u64) <= MAKER_LIVE_WITHIN_SLOTS {
                Liveness::Live
            } else {
                Liveness::Stale
            }
        }
        _ => Liveness::Unknown,
    }
}

/// Read `owner`'s base/quote ATA token balances for `market` into a
/// [`ParticipantView`]. A missing ATA reads as zero — the participant simply
/// holds none of that leg.
fn read_participant(client: &RpcClient, owner: &Pubkey, market: &MarketView) -> ParticipantView {
    let base_ata =
        chain::associated_token_address(owner, &market.base_mint, &chain::SPL_TOKEN_PROGRAM_ID);
    let quote_ata =
        chain::associated_token_address(owner, &market.quote_mint, &chain::SPL_TOKEN_PROGRAM_ID);
    let fetched = client.get_multiple_accounts(&[base_ata, quote_ata]).ok();
    // SPL Token account layout: mint(32) · owner(32) · amount(u64 LE) at 64.
    let amount = |i: usize| -> u64 {
        fetched
            .as_ref()
            .and_then(|v| v.get(i))
            .and_then(|o| o.as_ref())
            .and_then(|a| a.data.get(64..72))
            .and_then(|b| b.try_into().ok())
            .map(u64::from_le_bytes)
            .unwrap_or(0)
    };
    ParticipantView {
        address: *owner,
        base_tokens: amount(0),
        quote_tokens: amount(1),
        // Filled in by the caller for the leader; the default is the honest
        // answer for a participant with no observable liveness signal.
        liveness: Liveness::Unknown,
    }
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

/// Read the current on-chain reference price of `vault_idx` on `market` from
/// the slab — the anchor the eCLOB reprice control nudges. Read fresh at the
/// nudge (rather than carried on [`MarketView`]) so the bump is relative to
/// the live peg, not a poll-stale one. `None` if the market can't be read /
/// decoded or the sector isn't active.
pub fn read_reference_price(client: &RpcClient, market: &Pubkey, vault_idx: u32) -> Option<Price> {
    let account = client.get_account(market).ok()?;
    let view = SlabView::load(&account.data).ok()?;
    // The `active_vaults` iterator borrows `view` → `account.data`; a loop
    // drops it before the tail `None`, and the returned `Price` is owned, so
    // no borrow escapes the function.
    for (idx, vault) in view.active_vaults() {
        if idx == vault_idx {
            return Some(vault.reference_price.price());
        }
    }
    None
}

/// Load a specific market by address, for the multi-market bootstrap which
/// seeds each pair's own market PDA rather than whichever turns up first.
pub fn read_market_at(client: &RpcClient, address: Pubkey) -> Option<MarketView> {
    let slot = client.get_slot().ok()?;
    read_markets(client, slot.min(u32::MAX as u64) as u32, Some(address))
        .into_iter()
        .next()
}

/// Discover the localnet markets by scanning the program's owned accounts for
/// the `MarketHeader` discriminator, decoding each one's header + active vault
/// list via the slab-layout mirror, and reconstructing its resting book at
/// `current_slot` through the shared SDK matcher. With `target` set, only that
/// exact market is returned (the by-address bootstrap path); otherwise every
/// market the scan turns up, in scan order.
///
/// The single program-accounts scan is shared across every market — each one's
/// open depositors are filtered from it — so N markets cost one `get_program_accounts`
/// plus a small `get_multiple_accounts` per market for the SPL-owned treasuries
/// and mints (not in the program scan).
fn read_markets(client: &RpcClient, current_slot: u32, target: Option<Pubkey>) -> Vec<MarketView> {
    let Ok(accounts) = client.get_program_accounts(&DROPSET_ID) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (address, account) in &accounts {
        let is_market = account.data.len() >= 8 && account.data[..8] == MARKET_HEADER_DISCRIMINATOR;
        if !is_market || target.is_some_and(|t| *address != t) {
            continue;
        }
        let Ok(view) = SlabView::load(&account.data) else {
            continue;
        };
        let header = view.header;
        let base_mint = Pubkey::new_from_array(header.base_mint);
        let quote_mint = Pubkey::new_from_array(header.quote_mint);
        let base_treasury = Pubkey::new_from_array(header.base_treasury);
        let quote_treasury = Pubkey::new_from_array(header.quote_treasury);
        // Walk the active vaults once, collecting the teardown roster and, from
        // the first one (the vault the accounts pane surfaces as the MM bot),
        // its quote slot for the leader's liveness.
        let mut live_vaults: Vec<(u32, Pubkey)> = Vec::new();
        let mut leader_quote_slot: Option<u32> = None;
        let mut leader_reference: Option<Price> = None;
        for (idx, v) in view.active_vaults() {
            if live_vaults.is_empty() {
                leader_quote_slot = Some(v.reference_price.quote_slot.get());
                let p = v.reference_price.price();
                if p.is_valid() && !p.is_zero() && !p.is_infinity() {
                    leader_reference = Some(p);
                }
            }
            live_vaults.push((idx, Pubkey::new_from_array(v.leader)));
        }

        // Reconstruct the resting book via the shared matcher (Buy ⇒ asks,
        // Sell ⇒ bids) — the same levels a real swap would fill.
        let asks = resting_levels(&view, SwapSide::Buy, current_slot);
        let bids = resting_levels(&view, SwapSide::Sell, current_slot);

        // Open VaultDepositor PDAs for this market — discovered in the same
        // program-accounts scan, decoded for their (sector, owner).
        let depositors: Vec<(u32, Pubkey)> = accounts
            .iter()
            .filter(|(_, a)| {
                a.data.len() >= 8 && a.data[..8] == VAULT_DEPOSITOR_HEADER_DISCRIMINATOR
            })
            .filter_map(|(_, a)| VaultDepositorHeader::from_bytes(&a.data).ok())
            .filter(|h| h.market == *address)
            .map(|h| (h.sector_idx, h.owner))
            .collect();

        // Treasury ATAs and mints are SPL-owned, so they aren't in the
        // program-accounts scan — read them directly: the treasuries for their
        // lamports, the mints for their `decimals` (byte 44 of an SPL Mint).
        let Ok(fetched) =
            client.get_multiple_accounts(&[base_treasury, quote_treasury, base_mint, quote_mint])
        else {
            continue;
        };
        let at = |i: usize| fetched.get(i).and_then(|o| o.as_ref());
        let lamports = |i: usize| at(i).map_or(0, |a| a.lamports);
        let decimals = |i: usize| at(i).and_then(|a| a.data.get(44).copied()).unwrap_or(0);
        let base_decimals = decimals(2);
        let quote_decimals = decimals(3);
        // Scale the leader's atoms-ratio reference to human quote-per-base with
        // the pair's decimals, so the markets pane shows the fair value the
        // maker pegs to (distinct from the reconstructed book mid).
        let reference_price = leader_reference.map(|p| {
            p.quote_for_base(10u64.pow(base_decimals as u32)) as f64
                / 10f64.powi(quote_decimals as i32)
        });

        out.push(MarketView {
            address: *address,
            lamports: account.lamports,
            base_mint,
            quote_mint,
            base_treasury,
            quote_treasury,
            base_treasury_lamports: lamports(0),
            quote_treasury_lamports: lamports(1),
            active_count: header.active_count.get(),
            live_vaults,
            leader_quote_slot,
            reference_price,
            depositors,
            base_decimals,
            quote_decimals,
            asks,
            bids,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal market carrying just the `active_count` and an identifying
    /// base mint the phase / selection logic reads.
    fn market(active_count: u32, base: u8) -> MarketView {
        MarketView {
            address: Pubkey::new_from_array([base; 32]),
            lamports: 0,
            base_mint: Pubkey::new_from_array([base; 32]),
            quote_mint: Pubkey::default(),
            base_treasury: Pubkey::default(),
            quote_treasury: Pubkey::default(),
            base_treasury_lamports: 0,
            quote_treasury_lamports: 0,
            active_count,
            live_vaults: Vec::new(),
            leader_quote_slot: None,
            reference_price: None,
            depositors: Vec::new(),
            base_decimals: 6,
            quote_decimals: 6,
            asks: Vec::new(),
            bids: Vec::new(),
        }
    }

    /// A bootstrapped-enough state (validator up, program + registry present)
    /// carrying `markets`, so `phase()` turns purely on the market aggregate.
    fn ready_state(markets: Vec<MarketView>) -> ChainState {
        ChainState {
            validator_up: true,
            program_deployed: true,
            registry: Some(RegistryView {
                address: Pubkey::default(),
                lamports: 0,
                fee_mint: Pubkey::default(),
                fee_token_program: Pubkey::default(),
                fee_vault: Pubkey::default(),
                fee_vault_lamports: 0,
                market_count: markets.len() as u32,
            }),
            markets,
            ..Default::default()
        }
    }

    #[test]
    fn phase_is_ready_only_when_every_market_has_a_live_vault() {
        // No markets → still awaiting market creation.
        assert_eq!(ready_state(Vec::new()).phase(), Phase::MarketAbsent);
        // Markets exist but not all seeded → vault phase (the bootstrap seeds
        // them all together, so a single unseeded market gates the aggregate).
        assert_eq!(
            ready_state(vec![market(1, 1), market(0, 2)]).phase(),
            Phase::VaultAbsent
        );
        // Every market has a live vault → ready.
        assert_eq!(
            ready_state(vec![market(1, 1), market(2, 2)]).phase(),
            Phase::Ready
        );
    }

    #[test]
    fn maker_liveness_tracks_quote_freshness() {
        // A recent quote is live; the boundary slot is inclusive.
        assert_eq!(maker_liveness(Some(1_000), Some(1_000)), Liveness::Live);
        assert_eq!(
            maker_liveness(Some(1_000 + MAKER_LIVE_WITHIN_SLOTS), Some(1_000)),
            Liveness::Live
        );
        // One slot past the window is stale.
        assert_eq!(
            maker_liveness(Some(1_001 + MAKER_LIVE_WITHIN_SLOTS), Some(1_000)),
            Liveness::Stale
        );
        // A vault that has never quoted (`quote_slot == 0`) reads as stale.
        assert_eq!(maker_liveness(Some(1_000_000), Some(0)), Liveness::Stale);
        // A future-dated quote (clock skew) saturates to zero age, not stale.
        assert_eq!(maker_liveness(Some(10), Some(1_000)), Liveness::Live);
        // No head slot (validator down) or no live vault → unknown.
        assert_eq!(maker_liveness(None, Some(1_000)), Liveness::Unknown);
        assert_eq!(maker_liveness(Some(1_000), None), Liveness::Unknown);
    }

    #[test]
    fn selected_market_clamps_out_of_range_to_the_first() {
        let state = ready_state(vec![market(1, 1), market(1, 2)]);
        assert_eq!(
            state.selected_market(1).unwrap().base_mint,
            [2u8; 32].into()
        );
        // An index past the end falls back to the first rather than `None`.
        assert_eq!(
            state.selected_market(9).unwrap().base_mint,
            [1u8; 32].into()
        );
        // With no markets there is nothing to select.
        assert!(ready_state(Vec::new()).selected_market(0).is_none());
    }
}
