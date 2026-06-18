//! Structured events emitted on cold paths (open / deposit / withdraw /
//! realize) and on the taker hot path (`FillEvent`, per-leg).
//!
//! Per the architecture spec's **Events and emission**, the cold-path
//! events use the default `#[event]` (wincode / borsh-compatible) so
//! they can carry variable-shape data; `FillEvent` uses
//! `#[event(bytemuck)]` because it is fixed-size by construction and
//! lives on the hot path where the zero serializer cost matters.

use anchor_lang_v2::prelude::*;

use crate::state::RealizeOutcome;
use crate::Price;

/// Emitted by `create_vault`.
#[event]
pub struct CreateVaultEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub leader: Address,
    pub quote_authority: Address,
    pub perf_fee_rate: u32,
    pub min_leader_share: u32,
    pub allow_outside_depositors: bool,
}

/// Emitted by `close_vault` when a leader moves their vault from the
/// active DLL to the tombstone DLL. Matching stops; depositor flows
/// stay open until the vault drains. See the spec's **CloseVault**.
#[event]
pub struct CloseVaultEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub leader: Address,
    /// Active-DLL length after the move.
    pub active_count_after: u32,
}

/// Emitted by `freeze_vault` when an admin freezes a vault. The vault
/// stays on the active DLL (existing levels still match until expiry)
/// but can no longer be re-quoted. See the spec's **FreezeVault**.
#[event]
pub struct FreezeVaultEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub leader: Address,
}

/// Emitted by `set_min_leader_share` when an admin retunes a vault's
/// skin-in-the-game floor after creation. See the spec's
/// **SetMinLeaderShare**.
#[event]
pub struct SetMinLeaderShareEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub min_leader_share: u32,
}

/// Emitted by `set_market_fee_config` when an admin retunes a market's
/// per-`CreateVault` fee. Load-bearing for teardown: the chain does not
/// enumerate the set of historical fee mints, so the admin reconstructs
/// it off-chain from these events to sweep every fee ATA. See the spec's
/// **SetMarketFeeConfig** and **Account lifecycle and rent reclamation**.
#[event]
pub struct SetMarketFeeConfigEvent {
    pub market: Address,
    pub mint: Address,
    pub token_program: Address,
    pub atoms: u64,
}

/// Emitted by `set_taker_fee` when an admin retunes a market's taker
/// fee (ppm, [`crate::Ppm16`]) after creation. The fee is read on the
/// swap hot path; this is the only lever that moves it post-`create_market`.
/// See the spec's **SetTakerFee**.
#[event]
pub struct SetTakerFeeEvent {
    pub market: Address,
    pub taker_fee: u16,
}

/// Emitted by `set_registry_defaults` when an admin retunes the
/// registry-wide defaults stamped onto *future* markets. Carries the
/// resulting values of every default the instruction can touch — not
/// just the fields changed on this call — so an indexer sees the full
/// post-update default set regardless of which `Option`s were supplied.
/// Existing markets are unaffected. See the spec's **SetRegistryDefaults**.
#[event]
pub struct SetRegistryDefaultsEvent {
    pub default_taker_fee: u16,
    pub default_min_leader_share: u32,
}

/// Emitted by `deposit` after share accounting + basis math.
#[event]
pub struct DepositEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub depositor: Address,
    pub is_leader: bool,
    pub is_seeding: bool,
    pub base_in: u64,
    pub quote_in: u64,
    pub shares_out: u64,
    pub total_shares_after: u64,
    pub leader_shares_after: u64,
    pub base_atoms_after: u64,
    pub quote_atoms_after: u64,
}

/// Emitted by `withdraw` after share burn + basis crystallization.
#[event]
pub struct WithdrawEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub depositor: Address,
    pub is_leader: bool,
    pub shares_in: u64,
    pub base_out: u64,
    pub quote_out: u64,
    pub total_shares_after: u64,
    pub leader_shares_after: u64,
    pub base_atoms_after: u64,
    pub quote_atoms_after: u64,
    /// Signed PnL delta crystallized on this withdrawal (outside path).
    pub realized_pnl_delta: i64,
}

/// Emitted by `deposit` / `withdraw` whenever the implicit `Realize`
/// step mints new shares to the leader. Per spec, the hot path never
/// touches `Realize`, so swap does not emit this.
#[event]
pub struct RealizeEvent {
    pub market: Address,
    pub sector_idx: u32,
    pub shares_minted: u64,
    pub leader_shares_after: u64,
    pub total_shares_after: u64,
    pub hwm_after: u64,
}

impl RealizeEvent {
    /// Build the conditional `RealizeEvent` shared by every deposit /
    /// withdraw handler. The implicit `Realize` step only mints shares
    /// when `VPS` clears the high-water mark, so each handler emits
    /// `Some` solely on that case and `None` otherwise. `shares_minted`
    /// and `hwm_after` come off the `realize_in_place` outcome; the
    /// caller supplies its post-state `leader_shares_after` /
    /// `total_shares_after` (the local share totals differ per handler).
    pub fn from_outcome(
        outcome: &RealizeOutcome,
        market: Address,
        sector_idx: u32,
        leader_shares_after: u64,
        total_shares_after: u64,
    ) -> Option<Self> {
        (outcome.shares_minted > 0).then_some(RealizeEvent {
            market,
            sector_idx,
            shares_minted: outcome.shares_minted,
            leader_shares_after,
            total_shares_after,
            hwm_after: outcome.hwm_after,
        })
    }
}

/// Per-leg fill record. Bytemuck-serialized via `emit_cpi!` so the
/// inner-instruction data carries the canonical trade record at
/// `~1000 CU` + payload size per emit — the hot path can afford it.
///
/// Spec § **Events and emission → Granularity**: every leg is recorded,
/// no truncation. A sweep that exceeds one self-CPI's instruction-data
/// budget splits across multiple `emit_cpi!` calls.
#[event(bytemuck)]
pub struct FillEvent {
    pub market: Address,
    pub taker: Address,
    pub leader: Address,
    pub quote_authority: Address,
    /// `0` for ask-side (taker Buy), `1` for bid-side (taker Sell).
    pub side: u8,
    /// Padding so subsequent fields are aligned-1-friendly without
    /// implicit struct padding the bytemuck check would reject.
    pub _pad: [u8; 7],
    pub sector_idx: u32,
    pub level_idx: u32,
    pub fill_base: u64,
    pub fill_quote: u64,
    pub fill_price: Price,
    /// Padding to keep the next `u64` at an 8-byte boundary in the
    /// fixed-size repr-C layout. `Price` is `u32`, so 4 bytes of pad
    /// keep the struct stride well-defined.
    pub _pad2: [u8; 4],
    pub base_atoms_after: u64,
    pub quote_atoms_after: u64,
    pub nonce_after: u64,
    pub taker_fee_atoms: u64,
}
