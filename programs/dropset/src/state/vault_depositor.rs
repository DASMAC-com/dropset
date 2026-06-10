//! Per-`(vault, owner)` outside-depositor record.
//!
//! PDA seeds: `["vault_depositor", market, sector_idx_le, owner]`. Sector
//! reuse is safe — by spec invariant I6 a sector reaches the free list
//! only when `total_shares == 0`, which requires every `VaultDepositor`
//! on it to have already been closed. The next deposit into a recycled
//! sector re-derives the same address and `init_if_needed` allocates a
//! fresh PDA. See the architecture spec's **Depositor positions and
//! cost basis**.

use anchor_lang_v2::prelude::*;

use crate::Price;

/// Authoritative on-chain record of one depositor's position in one
/// vault — both the share claim and the cost basis the unrealized PnL
/// is measured against. All fields are alignment-1 so the struct casts
/// directly from the account bytes.
#[account]
pub struct VaultDepositorHeader {
    /// Market PDA this position is on. The vault is identified by
    /// `(market, sector_idx)`; the PDA seeds include both so a sector
    /// reuse across vault lifetimes derives a fresh address.
    pub market: Address,
    /// Sector index within the market's slab.
    pub sector_idx: PodU32,
    /// Depositor wallet — bound by the PDA seeds, so the account is
    /// non-transferable (there is no separate authority field to
    /// reassign). See the spec's **Shares**.
    pub owner: Address,
    /// Pro-rata claim on the vault — the per-depositor term of
    /// invariant I6 (`leader_shares + Σ VaultDepositor.shares ==
    /// total_shares`).
    pub shares: PodU64,
    /// Quote-denominated principal of the **remaining** position. Spec
    /// formula (`Deposit` top-off): `Σ (quote_in + base_in × ref_now)`.
    /// Reduced on `Withdraw` by the floored share-pro-rata slice.
    pub net_deposits: PodU64,
    /// Lifetime contributions, monotonic — **never reduced on
    /// withdraw**. Stable denominator for all-time return %.
    pub gross_deposited: PodU64,
    /// Shares-weighted average reference price across deposits.
    pub entry_ref_price: Price,
    /// Shares-weighted average VPS (`L / total_shares`) across deposits,
    /// Q32.32. Same encoding and practical bound as [`crate::Vault::hwm`].
    pub entry_vps: PodU64,
    /// Slot of the first deposit. Captured once at PDA init.
    pub opened_at: PodU64,
    /// Signed quote-denominated PnL crystallized by past withdrawals.
    /// Discarded when the account is closed at zero shares.
    pub realized_pnl: PodI64,
    /// Signed yield (ex-FX) component of [`Self::realized_pnl`].
    pub realized_yield: PodI64,
    /// Signed FX component. Invariant:
    /// `realized_yield + realized_fx == realized_pnl`.
    pub realized_fx: PodI64,
    /// PDA bump.
    pub bump: u8,
    /// Padding so the on-chain size is a multiple of 8 and the
    /// `#[account]` derive's discriminator + body length matches the
    /// const-asserted total below.
    pub _reserved: [u8; 7],
}

// Pin the on-chain layout — same offset-guard pattern as `MarketHeader`
// / `Vault`. A reorder that preserves the total size would silently
// shift fields without these.
const _: () = assert!(core::mem::size_of::<VaultDepositorHeader>() == 144);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, market) == 0);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, sector_idx) == 32);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, owner) == 36);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, shares) == 68);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, bump) == 136);
const _: () = assert!(core::mem::offset_of!(VaultDepositorHeader, _reserved) == 137);

/// PDA seed prefix for [`VaultDepositorHeader`] accounts.
pub const VAULT_DEPOSITOR_SEED: &[u8] = b"vault_depositor";
