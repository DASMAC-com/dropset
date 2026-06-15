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

impl VaultDepositorHeader {
    /// Establish or extend this depositor's cost basis after a deposit of
    /// `shares_out` shares carrying `lot_quote_value` quote-denominated
    /// principal, entering at `vps_after` (Q32.32) and reference price
    /// `ref_now`.
    ///
    /// Owns the basis invariants for every field this type holds
    /// (`shares`, `net_deposits`, `gross_deposited`, `entry_ref_price`,
    /// `entry_vps`, `opened_at`) so the handler no longer carries the
    /// first-deposit-vs-top-off branching inline. `realized_*` and `bump`
    /// are untouched — they default at PDA init / are stamped by the
    /// handler.
    ///
    /// Returns `true` when this was the **first** deposit into the PDA,
    /// signalling the caller to bump `Market::outstanding_vault_depositors`
    /// — that counter is `Market` state, not depositor state, and stays in
    /// the handler.
    ///
    /// On a top-off it shares-weighted-merges `entry_vps` (Q32.32, so a raw
    /// u128 weighted average is exact) and routes `entry_ref_price` through
    /// [`Price::weighted_average`] (a custom decimal-float that must
    /// decode / average / re-encode). See the spec's **Depositor positions
    /// and cost basis → Top-off**.
    #[allow(clippy::too_many_arguments)]
    pub fn record_deposit(
        &mut self,
        market: Address,
        sector_idx: u32,
        owner: Address,
        shares_out: u64,
        lot_quote_value: u64,
        vps_after: u64,
        ref_now: Price,
        opened_at: u64,
    ) -> bool {
        let prior_shares = self.shares.get();
        let new_shares = prior_shares + shares_out;
        if prior_shares == 0 {
            // First deposit into this PDA — stamp all basis fields.
            self.market = market;
            self.sector_idx = sector_idx.into();
            self.owner = owner;
            self.shares = new_shares.into();
            self.net_deposits = lot_quote_value.into();
            self.gross_deposited = lot_quote_value.into();
            self.entry_ref_price = ref_now;
            self.entry_vps = vps_after.into();
            self.opened_at = opened_at.into();
            true
        } else {
            // Top-off: merge shares-weighted averages.
            let s = prior_shares as u128;
            let ds = shares_out as u128;
            let denom = s + ds;
            let entry_vps_prev = self.entry_vps.get() as u128;
            let entry_vps_new = (s * entry_vps_prev + ds * (vps_after as u128)) / denom;
            let entry_ref_new = self.entry_ref_price.weighted_average(ref_now, s, ds);
            self.shares = new_shares.into();
            self.net_deposits = (self.net_deposits.get() + lot_quote_value).into();
            self.gross_deposited = (self.gross_deposited.get() + lot_quote_value).into();
            self.entry_vps = (entry_vps_new as u64).into();
            self.entry_ref_price = entry_ref_new;
            false
        }
    }
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

// PDA seed prefix for `VaultDepositorHeader` accounts: the byte string
// `b"vault_depositor"`. It is inlined directly in each `seeds = [ ... ]`
// constraint (`deposit`, `withdraw`) rather than referenced through a
// named constant — anchor v2's IDL classifier recognizes a byte-string
// literal as `IdlSeed::Const` but treats a named-constant reference as
// the opaque `{"kind":"expr"}` fallback, which anchor CLI's
// `IdlInstructionAccountItem` deserializer then rejects. There is no
// runtime path that needs the seed as a value, so no constant is
// exported; if a client-helper crate ever needs one, define it there.
