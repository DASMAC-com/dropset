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
use anyhow::{anyhow, bail, Context, Result};
use dropset_sdk::accounts::{
    fetch_maybe_registry_header, VaultDepositorHeader, MARKET_HEADER_DISCRIMINATOR,
    VAULT_DEPOSITOR_HEADER_DISCRIMINATOR,
};
use dropset_sdk::clock::{SlotTime, WallTime};
use dropset_sdk::layout::{MarketView as SlabView, Vault};
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
    /// Every vault exists, but at least one holds no deposit yet.
    VaultUnseeded,
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
            Phase::VaultUnseeded => "Vault unseeded",
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
    /// The leader of every live vault that holds no deposit yet — opened,
    /// never seeded (or drained back to empty).
    pub unseeded_leaders: Vec<Pubkey>,
    /// The `reference_price.quote_slot` of the first live vault — the one whose
    /// leader the accounts pane shows as the MM bot. Drives the leader's
    /// liveness (freshness against the poll's head slot). `None` when the market
    /// has no live vault; `Some(0)` for a vault that has never quoted (reads as
    /// [`Liveness::Unknown`], not stale — see `maker_liveness`).
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
/// [`Liveness::Live`], one that has gone quiet reads [`Liveness::Stale`], and
/// one that has never quoted at all reads [`Liveness::Unknown`], independent
/// of who launched it. The taker leaves no such on-chain footprint
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
    /// The market PDAs the session's roster expects. Not read from the chain —
    /// the caller sets it after [`poll`] — but stored here so [`Self::phase`]
    /// can measure progress against what *should* exist. Empty means no
    /// roster is known, and the phase falls back to the discovered markets.
    pub roster_markets: Vec<Pubkey>,
    /// The session's vault leader, set by the caller beside
    /// [`Self::roster_markets`]. When known, the vault phases count only
    /// **this** leader's vaults — the same key the ceremony steps check — so a
    /// vault someone else opened (`create_vault` is permissionless) neither
    /// greys out "Create vault" nor pins the phase at unseeded. `None` falls
    /// back to counting every vault.
    pub roster_leader: Option<Pubkey>,
}

impl ChainState {
    /// Derive the gating [`Phase`] from the snapshot. The bootstrap actions
    /// bring up every roster market together, so the phase is an aggregate:
    /// `Ready` only once every roster market exists with a live, seeded vault.
    ///
    /// Measured against the roster rather than against whatever was
    /// discovered, so a run that stopped part-way still reads as unfinished.
    /// Gated on the discovered list alone, one created market of three read as
    /// "market already exists" and greyed out the very step needed to finish —
    /// unrecoverable on mainnet, where there is no wipe. The phase is only a
    /// hint: each ceremony step re-checks the chain fresh before it sends.
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
        let tracked: Vec<&MarketView> = if self.roster_markets.is_empty() {
            self.markets.iter().collect()
        } else {
            if !self
                .roster_markets
                .iter()
                .all(|r| self.markets.iter().any(|m| m.address == *r))
            {
                return Phase::MarketAbsent;
            }
            self.markets
                .iter()
                .filter(|m| self.roster_markets.contains(&m.address))
                .collect()
        };
        if tracked.is_empty() {
            return Phase::MarketAbsent;
        }
        let has_vault = |m: &MarketView| match self.roster_leader {
            Some(leader) => m.live_vaults.iter().any(|(_, l)| *l == leader),
            None => m.active_count > 0,
        };
        let unseeded = |m: &MarketView| match self.roster_leader {
            Some(leader) => m.unseeded_leaders.contains(&leader),
            None => !m.unseeded_leaders.is_empty(),
        };
        if tracked.iter().any(|m| !has_vault(m)) {
            Phase::VaultAbsent
        } else if tracked.iter().any(|m| unseeded(m)) {
            Phase::VaultUnseeded
        } else {
            Phase::Ready
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
    mint_symbols: &[(Pubkey, &'static str)],
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

    // The book's expiry filter is dual-domain: the poll's own `slot` (also
    // shown on the status line as a liveness signal) plus the host wall
    // clock, matching what the engine gates on.
    state.markets = read_markets(
        client,
        SlotTime::new(slot.unwrap_or(0).min(u32::MAX as u64) as u32),
        dropset_sdk::time::now_unix_u32(),
        None,
    );
    sort_markets_by_symbol(&mut state.markets, mint_symbols);
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
/// anything older is [`Liveness::Stale`]. Without a head slot (validator down)
/// there is nothing to compare against, so the result is [`Liveness::Unknown`].
///
/// A vault that has **never** quoted carries `quote_slot == 0`, which is not a
/// stale quote but the absence of one, so it reads [`Liveness::Unknown`] too.
/// The distinction is the whole point of the two variants: `Stale` says a
/// maker is there and has fallen behind — booting, wedged, or stopped — and
/// invites you to go look at it, while `Unknown` says nothing has been
/// observed. Since markets open dark, a freshly bootstrapped demo has
/// `quote_slot == 0` on every market, so reading that as `Stale` would show a
/// yellow "quotes have aged" dot for a maker that was never launched — the
/// default opening state, not a corner case.
fn maker_liveness(head_slot: Option<u64>, quote_slot: Option<u32>) -> Liveness {
    match (head_slot, quote_slot) {
        // Never quoted — no signal, not an aged one.
        (_, Some(0)) => Liveness::Unknown,
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
    // Expiry is dual-domain, so the book filter needs both clocks: the
    // chain's slot and the host's wall clock.
    let slot = client.get_slot().ok()?;
    read_markets(
        client,
        SlotTime::new(slot.min(u32::MAX as u64) as u32),
        dropset_sdk::time::now_unix_u32(),
        Some(address),
    )
    .into_iter()
    .next()
}

/// One live vault on a market, as a pre-flight check reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VaultSeat {
    /// Its sector index — valid only for this read, since sectors recycle.
    /// What an instruction addresses the vault by.
    pub idx: u32,
    /// Its per-market vault number, stamped at `create_vault`: with the
    /// market, the vault's durable identity — what a person should be told,
    /// since a reused sector gets a new one.
    pub seq: u64,
    pub leader: Pubkey,
    /// Whether it holds a deposit (any shares or inventory).
    pub seeded: bool,
    /// The share ledger and inventory, as of this read — what the leader
    /// deposit / withdraw commands size their basket and slippage bounds on.
    pub stake: VaultStake,
}

/// A vault's share ledger and inventory at one read. Pre-realize: the
/// program accrues the performance fee before a withdraw, which can only
/// mint shares, so a slice sized on this read errs high — the slippage
/// tolerance absorbs it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VaultStake {
    pub total_shares: u64,
    pub leader_shares: u64,
    pub base_atoms: u64,
    pub quote_atoms: u64,
}

/// Whether a vault holds any deposit at all. Shares, not just inventory, so a
/// vault whose inventory was fully withdrawn but whose share ledger still
/// carries a balance does not read as fresh.
fn is_seeded(v: &Vault) -> bool {
    v.total_shares.get() > 0 || v.base_atoms.get() > 0 || v.quote_atoms.get() > 0
}

/// The registry, read **fresh** for a pre-flight check: `Ok(None)` only when
/// the account is genuinely absent, and an error — never `None` — when the read
/// fails. See [`chain::fetch_fresh`] for why the distinction is load-bearing.
pub fn registry_fresh(client: &RpcClient) -> Result<Option<RegistryView>> {
    if chain::fetch_fresh(client, &chain::registry_pda(), "registry")?.is_none() {
        return Ok(None);
    }
    read_registry(client)
        .map(Some)
        .context("the registry exists but could not be read back — refusing")
}

/// Every live vault on `market`, read **fresh**: `Ok(None)` when the market
/// account is absent, an error when it cannot be read or decoded.
///
/// A direct read of the one account rather than [`read_market_at`]'s
/// program-wide scan, which both folds a failure into `None` and costs a
/// `get_program_accounts` on mainnet for what is a single-account question.
pub fn vault_seats_fresh(client: &RpcClient, market: &Pubkey) -> Result<Option<Vec<VaultSeat>>> {
    let Some(account) = chain::fetch_fresh(client, market, "market")? else {
        return Ok(None);
    };
    if account.owner != DROPSET_ID {
        bail!(
            "{market} is owned by {}, not the Dropset program",
            account.owner
        );
    }
    let view = SlabView::load(&account.data)
        .map_err(|_| anyhow!("{market} does not decode as a market slab"))?;
    Ok(Some(
        view.active_vaults()
            .map(|(idx, v)| VaultSeat {
                idx,
                seq: v.seq.get(),
                leader: Pubkey::new_from_array(v.leader),
                seeded: is_seeded(v),
                stake: VaultStake {
                    total_shares: v.total_shares.get(),
                    leader_shares: v.leader_shares.get(),
                    base_atoms: v.base_atoms.get(),
                    quote_atoms: v.quote_atoms.get(),
                },
            })
            .collect(),
    ))
}

/// The seats in `seats` that `leader` leads, in sector order.
pub fn seats_led_by(seats: &[VaultSeat], leader: &Pubkey) -> Vec<VaultSeat> {
    seats
        .iter()
        .filter(|s| s.leader == *leader)
        .copied()
        .collect()
}

/// Discover the localnet markets by scanning the program's owned accounts for
/// the `MarketHeader` discriminator, decoding each one's header + active vault
/// list via the slab-layout mirror, and reconstructing its resting book at
/// `(now_slot, now_unix)` through the shared SDK matcher. With `target` set, only that
/// exact market is returned (the by-address bootstrap path); otherwise every
/// market the scan turns up, in scan order.
///
/// The single program-accounts scan is shared across every market — each one's
/// open depositors are filtered from it — so N markets cost one `get_program_accounts`
/// plus a small `get_multiple_accounts` per market for the SPL-owned treasuries
/// and mints (not in the program scan).
// The two clocks are domain-typed all the way from the caller (see
// `dropset_sdk::clock`), so the pair cannot be transposed into
// `resting_levels` below.
/// Order the markets list alphabetically by base-token symbol.
///
/// `get_program_accounts` returns accounts in the node's own store order,
/// which is arbitrary and — worse — **unstable**: it can differ between runs
/// and shift as accounts are written, so the list a reader just learned is not
/// the list they get next time. Sorting here rather than at render time is
/// deliberate: `poll` reads the selected market's participants by **index**
/// further down, so a list reordered after the fact would attribute one
/// market's holdings to another.
///
/// A market whose mint is not in the roster map sorts last rather than under
/// its placeholder glyph, which keeps an unknown market visible at a
/// predictable end of the list instead of interleaved among named ones. Ties
/// break on the address so the order is total even then.
fn sort_markets_by_symbol(markets: &mut [MarketView], mint_symbols: &[(Pubkey, &'static str)]) {
    let symbol_of = |mint: &Pubkey| {
        mint_symbols
            .iter()
            .find(|(m, _)| m == mint)
            .map(|(_, s)| *s)
    };
    markets.sort_by(|a, b| {
        let (sa, sb) = (symbol_of(&a.base_mint), symbol_of(&b.base_mint));
        // `Option`'s own ordering puts `None` FIRST, so this explicit
        // `is_none()` stage is what makes an unknown market sort last — do
        // not remove it as redundant. (An earlier version of this comment
        // claimed the opposite, which would have read as license to delete
        // the stage and silently invert unknown-last to unknown-first.)
        sa.is_none()
            .cmp(&sb.is_none())
            .then_with(|| sa.cmp(&sb))
            .then_with(|| a.address.cmp(&b.address))
    });
}

fn read_markets(
    client: &RpcClient,
    now_slot: SlotTime,
    now_unix: WallTime,
    target: Option<Pubkey>,
) -> Vec<MarketView> {
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
        let mut unseeded_leaders: Vec<Pubkey> = Vec::new();
        let mut leader_quote_slot: Option<u32> = None;
        let mut leader_reference: Option<Price> = None;
        for (idx, v) in view.active_vaults() {
            if live_vaults.is_empty() {
                leader_quote_slot = Some(v.reference_price.quote_slot.get());
                // The matcher's own gate, shared rather than re-derived, so the
                // pane reads "no reference" exactly when the engine would skip
                // the vault — including when the maker has killed its own book
                // by stamping the zero sentinel.
                let p = v.reference_price.price();
                if p.is_matchable() {
                    leader_reference = Some(p);
                }
            }
            live_vaults.push((idx, Pubkey::new_from_array(v.leader)));
            if !is_seeded(v) {
                unseeded_leaders.push(Pubkey::new_from_array(v.leader));
            }
        }

        // Reconstruct the resting book via the shared matcher (Buy ⇒ asks,
        // Sell ⇒ bids) — the same levels a real swap would fill.
        let asks = resting_levels(&view, SwapSide::Buy, now_slot, now_unix);
        let bids = resting_levels(&view, SwapSide::Sell, now_slot, now_unix);

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
        // maker pegs to (distinct from the reconstructed book mid). Shared with
        // the depth ladder rather than open-coded: the conversion has to probe
        // the ratio well above one base unit to survive a low-decimal token,
        // and one owner is what keeps the two panes from disagreeing.
        let reference_price =
            leader_reference.map(|p| crate::book::human_price(p, base_decimals, quote_decimals));

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
            unseeded_leaders,
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
            unseeded_leaders: Vec::new(),
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

    /// The list is alphabetical by symbol, and an unknown mint sorts last
    /// rather than interleaving under its placeholder glyph.
    ///
    /// Seeded in a deliberately scrambled order, because the defect this
    /// closes is that `get_program_accounts` returns whatever the node's
    /// store iteration yields — so the input order carries no information and
    /// the test must not accidentally depend on it.
    #[test]
    fn markets_sort_alphabetically_with_unknowns_last() {
        let symbols: Vec<(Pubkey, &'static str)> = vec![
            (Pubkey::new_from_array([3; 32]), "EURC"),
            (Pubkey::new_from_array([1; 32]), "CADC"),
            (Pubkey::new_from_array([2; 32]), "AUDD"),
        ];
        // 4 is absent from the map — a market discovered on-chain that the
        // bootstrap roster does not name.
        let mut markets = vec![market(1, 3), market(1, 4), market(1, 1), market(1, 2)];
        sort_markets_by_symbol(&mut markets, &symbols);

        let order: Vec<Pubkey> = markets.iter().map(|m| m.base_mint).collect();
        assert_eq!(
            order,
            vec![
                Pubkey::new_from_array([2; 32]), // AUDD
                Pubkey::new_from_array([1; 32]), // CADC
                Pubkey::new_from_array([3; 32]), // EURC
                Pubkey::new_from_array([4; 32]), // unknown, last
            ]
        );
    }

    /// The order must be total, so two runs over differently-shuffled input
    /// agree. Without this the sort could look right on one arrangement and
    /// still leave the list shifting between polls, which is the actual
    /// complaint.
    #[test]
    fn the_market_order_is_independent_of_the_input_order() {
        let symbols: Vec<(Pubkey, &'static str)> = vec![
            (Pubkey::new_from_array([1; 32]), "CADC"),
            (Pubkey::new_from_array([2; 32]), "AUDD"),
        ];
        let mut forward = vec![market(1, 1), market(1, 2), market(1, 9)];
        let mut reverse = vec![market(1, 9), market(1, 2), market(1, 1)];
        sort_markets_by_symbol(&mut forward, &symbols);
        sort_markets_by_symbol(&mut reverse, &symbols);
        let key = |ms: &[MarketView]| ms.iter().map(|m| m.base_mint).collect::<Vec<_>>();
        assert_eq!(key(&forward), key(&reverse));
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
        // A vault that exists but holds nothing yet gates on the deposit step.
        let mut unseeded = market(1, 2);
        unseeded.unseeded_leaders = vec![Pubkey::new_unique()];
        assert_eq!(
            ready_state(vec![market(1, 1), unseeded]).phase(),
            Phase::VaultUnseeded
        );
    }

    #[test]
    fn phase_measures_progress_against_the_roster() {
        // One of two roster markets created: still awaiting market creation,
        // so the step that finishes the job stays enabled. Gated on the
        // discovered list alone this read "vault absent" and stranded the run.
        let mut state = ready_state(vec![market(0, 1)]);
        state.roster_markets = vec![
            Pubkey::new_from_array([1; 32]),
            Pubkey::new_from_array([2; 32]),
        ];
        assert_eq!(state.phase(), Phase::MarketAbsent);
        // Both present; a foreign market with no vault does not hold it back,
        // and a roster market with no vault does.
        let mut state = ready_state(vec![market(1, 1), market(1, 2), market(0, 9)]);
        state.roster_markets = vec![
            Pubkey::new_from_array([1; 32]),
            Pubkey::new_from_array([2; 32]),
        ];
        assert_eq!(state.phase(), Phase::Ready);
        state.markets[1].active_count = 0;
        assert_eq!(state.phase(), Phase::VaultAbsent);
    }

    #[test]
    fn seats_led_by_selects_only_the_leaders_vaults() {
        let me = Pubkey::new_unique();
        let other = Pubkey::new_unique();
        let seats = [
            VaultSeat {
                idx: 0,
                seq: 1,
                leader: other,
                seeded: true,
                stake: VaultStake::default(),
            },
            VaultSeat {
                idx: 3,
                seq: 2,
                leader: me,
                seeded: false,
                stake: VaultStake::default(),
            },
        ];
        assert_eq!(seats_led_by(&seats, &me), vec![seats[1]]);
        assert!(seats_led_by(&seats, &Pubkey::new_unique()).is_empty());
        // Two seats under one leader both come back, in order — the shape the
        // deposit step must refuse as ambiguous rather than pick from.
        let doubled = [seats[1], VaultSeat { idx: 7, ..seats[1] }];
        assert_eq!(seats_led_by(&doubled, &me).len(), 2);
    }

    #[test]
    fn phase_counts_only_the_session_leaders_vaults() {
        // `create_vault` is permissionless, so a stranger can open a vault on a
        // roster market. Counted, it greyed out "Create vault" for a leader
        // who had none — unrecoverable on mainnet.
        let me = Pubkey::new_unique();
        let stranger = Pubkey::new_unique();
        let mut theirs = market(1, 1);
        theirs.live_vaults = vec![(0, stranger)];
        let mut state = ready_state(vec![theirs]);
        state.roster_markets = vec![Pubkey::new_from_array([1; 32])];
        state.roster_leader = Some(me);
        assert_eq!(state.phase(), Phase::VaultAbsent);
        // Their empty vault must not pin the phase at unseeded either, once
        // ours exists and is seeded.
        state.markets[0].live_vaults.push((1, me));
        state.markets[0].unseeded_leaders = vec![stranger];
        assert_eq!(state.phase(), Phase::Ready);
        // Ours unseeded is what gates on the deposit step.
        state.markets[0].unseeded_leaders.push(me);
        assert_eq!(state.phase(), Phase::VaultUnseeded);
        // Without a known leader, every vault counts (the old behavior).
        state.roster_leader = None;
        assert_eq!(state.phase(), Phase::VaultUnseeded);
    }

    #[test]
    fn a_vault_is_seeded_by_any_shares_or_inventory() {
        // The deposit step's never-twice guard rests on this predicate.
        use bytemuck::Zeroable;
        let mut v = Vault::zeroed();
        assert!(!is_seeded(&v));
        v.total_shares = 1u64.into();
        assert!(is_seeded(&v));
        let mut v = Vault::zeroed();
        v.base_atoms = 5u64.into();
        assert!(is_seeded(&v));
        let mut v = Vault::zeroed();
        v.quote_atoms = 5u64.into();
        assert!(is_seeded(&v));
    }

    #[test]
    fn fresh_reads_refuse_on_an_unreachable_endpoint() {
        let client = chain::rpc("http://127.0.0.1:1");
        assert!(registry_fresh(&client).is_err());
        assert!(vault_seats_fresh(&client, &Pubkey::new_unique()).is_err());
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
        // A vault that has never quoted (`quote_slot == 0`) has no signal at
        // all rather than an aged one, so it reads unknown, not stale — the
        // opening state of every market now that they bootstrap dark. It
        // stays unknown however far the head slot has advanced, and whether
        // or not a head slot was read.
        assert_eq!(maker_liveness(Some(1_000_000), Some(0)), Liveness::Unknown);
        assert_eq!(maker_liveness(Some(1), Some(0)), Liveness::Unknown);
        assert_eq!(maker_liveness(None, Some(0)), Liveness::Unknown);
        // One slot of quoting history is enough to distinguish the two: that
        // is a real quote, merely an old one.
        assert_eq!(maker_liveness(Some(1_000_000), Some(1)), Liveness::Stale);
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
