//! Structured events emitted on cold paths (open / deposit / withdraw /
//! realize) and on the taker hot path (`FillEvent`, per-leg).
//!
//! Per the architecture spec's **Events and emission**, the cold-path
//! events use the default `#[event]` (wincode / borsh-compatible) so
//! they can carry variable-shape data; `FillEvent` uses
//! `#[event(bytemuck)]` because it is fixed-size by construction and
//! lives on the hot path where the zero serializer cost matters.

use anchor_lang_v2::prelude::*;

use crate::Price;

/// Emitted by `register_vault` (spec's `OpenVault`).
#[event]
pub struct OpenVaultEvent {
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
