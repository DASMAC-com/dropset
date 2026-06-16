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

use crate::{errors::DropsetError, Price};

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

    /// Crystallize the realized PnL of withdrawing `shares_in` shares that
    /// deliver the `(slice_base, slice_quote)` basket, then burn the
    /// shares and reduce the cost basis. Returns the `realized_pnl` delta
    /// for the `WithdrawEvent`.
    ///
    /// Shared by `withdraw` (outside path) and `force_withdraw_depositor`
    /// so the program's most delicate accounting stays single-sourced; a
    /// divergent `min`/`max` or rounding change applied to only one copy
    /// would be high-impact and hard to spot. The caller validates the
    /// `VaultDepositor` identity and the slippage bounds before calling.
    ///
    /// The `shares >= shares_in` guard is reachable on the signed path
    /// (a depositor can request more than they hold) and defense-in-depth
    /// on the force path (which reads `shares_in` from `shares`, so the
    /// two are equal there today).
    ///
    /// Realized PnL math, per spec L1513-1519:
    ///   realized_fx    += slice_base × (ref_now − entry_ref)
    ///   realized_yield += slice_quote + slice_base × entry_ref − released_basis
    ///   realized_pnl   += slice_quote + slice_base × ref_now    − released_basis
    ///
    /// `ref_now × slice_base` and `entry_ref × slice_base` are decoded via
    /// [`Price::quote_for_base`] — both produce a quote-atom value, so the
    /// deltas are well-typed in quote-denominated units. All math is in
    /// `u128`/`i128` to avoid intermediate overflow; the signed
    /// accumulators clamp into `i64` at the store. `entry_vps`,
    /// `entry_ref_price`, and `gross_deposited` are intentionally left
    /// unchanged — a proportional reduction preserves the shares-weighted
    /// averages, and `gross_deposited` only ever grows (on deposit).
    pub fn crystallize_realized_pnl(
        &mut self,
        shares_in: u64,
        slice_base: u64,
        slice_quote: u64,
        ref_now: Price,
    ) -> Result<i64> {
        require!(
            self.shares.get() >= shares_in,
            DropsetError::InsufficientShares
        );
        let s_in = shares_in as u128;
        let released_basis = (self.net_deposits.get() as u128)
            .checked_mul(s_in)
            .ok_or(DropsetError::MathOverflow)?
            / (self.shares.get() as u128);
        let quote_for_ref_now = ref_now.quote_for_base(slice_base).min(i128::MAX as u128) as i128;
        let quote_for_ref_entry = self
            .entry_ref_price
            .quote_for_base(slice_base)
            .min(i128::MAX as u128) as i128;
        let slice_quote_i = slice_quote as i128;
        let released_i = released_basis as i128;
        let fx_delta: i128 = quote_for_ref_now.saturating_sub(quote_for_ref_entry);
        let yield_delta: i128 = slice_quote_i
            .saturating_add(quote_for_ref_entry)
            .saturating_sub(released_i);
        let pnl_delta: i128 = slice_quote_i
            .saturating_add(quote_for_ref_now)
            .saturating_sub(released_i);
        self.realized_fx = (((self.realized_fx.get() as i128).saturating_add(fx_delta))
            .clamp(i64::MIN as i128, i64::MAX as i128) as i64)
            .into();
        let new_pnl = (self.realized_pnl.get() as i128).saturating_add(pnl_delta);
        let new_yield = (self.realized_yield.get() as i128).saturating_add(yield_delta);
        self.realized_pnl = (new_pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64).into();
        self.realized_yield = (new_yield.clamp(i64::MIN as i128, i64::MAX as i128) as i64).into();

        // Burn the withdrawn shares and reduce the cost basis by the
        // released slice (`net_deposits' = net_deposits − released_basis`).
        self.shares = (self.shares.get() - shares_in).into();
        self.net_deposits = self
            .net_deposits
            .get()
            .checked_sub(released_basis as u64)
            .ok_or(DropsetError::MathOverflow)?
            .into();

        Ok(pnl_delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a bare `VaultDepositorHeader` carrying just the fields the
    /// crystallization math reads — `shares`, `net_deposits`, and
    /// `entry_ref_price`. Identity / averaging fields are irrelevant
    /// here and left zeroed; the `realized_*` accumulators start at 0.
    fn vd(shares: u64, net_deposits: u64, entry_ref: Price) -> VaultDepositorHeader {
        VaultDepositorHeader {
            market: Address::default(),
            sector_idx: 0u32.into(),
            owner: Address::default(),
            shares: shares.into(),
            net_deposits: net_deposits.into(),
            gross_deposited: net_deposits.into(),
            entry_ref_price: entry_ref,
            entry_vps: 0u64.into(),
            opened_at: 0u64.into(),
            realized_pnl: 0i64.into(),
            realized_yield: 0i64.into(),
            realized_fx: 0i64.into(),
            bump: 0,
            _reserved: [0u8; 7],
        }
    }

    /// Reference price `value = 1.0` — `quote_for_base(b) == b`.
    fn price_one() -> Price {
        Price::encode(10_000_000, 0).unwrap()
    }

    /// Reference price `value = 2.0` — `quote_for_base(b) == 2b`.
    fn price_two() -> Price {
        Price::encode(20_000_000, 0).unwrap()
    }

    #[test]
    fn crystallize_splits_fx_and_yield_and_reduces_basis() {
        // Enter at ref 1.0, withdraw half (50 of 100 shares) with the
        // reference now at 2.0 and an all-base slice of 100.
        //   released_basis = floor(1000 × 50 / 100)      = 500
        //   fx     = 2×100 − 1×100                        = +100
        //   yield  = 0 + 1×100 − 500                      = −400
        //   pnl    = 0 + 2×100 − 500                      = −300
        let mut v = vd(100, 1_000, price_one());
        let delta = v.crystallize_realized_pnl(50, 100, 0, price_two()).unwrap();
        assert_eq!(delta, -300, "returned delta is the realized_pnl move");
        assert_eq!(v.realized_fx.get(), 100);
        assert_eq!(v.realized_yield.get(), -400);
        assert_eq!(v.realized_pnl.get(), -300);
        // Invariant: realized_yield + realized_fx == realized_pnl.
        assert_eq!(
            v.realized_yield.get() + v.realized_fx.get(),
            v.realized_pnl.get()
        );
        // Shares burned and basis reduced by exactly the released slice.
        assert_eq!(v.shares.get(), 50);
        assert_eq!(v.net_deposits.get(), 500);
    }

    #[test]
    fn crystallize_full_drain_zeroes_shares_and_basis() {
        let mut v = vd(100, 1_000, price_one());
        v.crystallize_realized_pnl(100, 0, 1_000, price_one())
            .unwrap();
        assert_eq!(v.shares.get(), 0);
        assert_eq!(v.net_deposits.get(), 0);
        // Flat reference and slice_quote == basis → no realized move.
        assert_eq!(v.realized_pnl.get(), 0);
    }

    #[test]
    fn crystallize_accumulates_across_calls() {
        // Two successive partial withdrawals at a flat 1.0 reference.
        // Each: slice_quote = released_basis, slice_base = 0 → no move.
        // Then a profitable third leg pushes realized_pnl positive.
        let mut v = vd(100, 1_000, price_one());
        v.crystallize_realized_pnl(25, 0, 250, price_one()).unwrap();
        assert_eq!(v.shares.get(), 75);
        assert_eq!(v.net_deposits.get(), 750);
        // Reference doubled: an all-base slice of 100 against released
        // basis floor(750 × 25 / 75) = 250 → pnl = 2×100 − 250 = −50.
        let delta = v.crystallize_realized_pnl(25, 100, 0, price_two()).unwrap();
        assert_eq!(delta, -50);
        assert_eq!(v.realized_pnl.get(), -50);
        assert_eq!(v.shares.get(), 50);
        assert_eq!(v.net_deposits.get(), 500);
    }

    #[test]
    fn crystallize_rejects_overdraw() {
        let mut v = vd(10, 100, price_one());
        assert!(v.crystallize_realized_pnl(20, 0, 0, price_one()).is_err());
    }
}
