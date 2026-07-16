//! Thin program-side wrappers around the solana-free
//! [`dropset_math_core`] kernels — each reads/writes `&mut Vault` state
//! around a pure formula, so the layering contract holds and handlers stop
//! touching the physical slab layout:
//! - perf-fee realization ([`realize_in_place`], which honors the on-chain
//!   `frozen` / `tombstoned` flags);
//! - single-leg subsequent-deposit sizing ([`single_leg_basket`], which
//!   maps the kernel's [`BasketError`] back onto [`DropsetError`]);
//! - flush materialization ([`Vault::materialize_remaining`], which
//!   rebuilds [`Vault::remaining`] from [`Vault::profile`] + current
//!   inventory via the [`matching_math`](dropset_math_core::matching_math)
//!   kernels and clears `FLUSH_BIT`).
//!
//! The pure formulas live in `dropset-math-core` so the off-chain NAV and
//! simulator consumers reuse them.

use anchor_lang_v2::prelude::*;
use dropset_math_core::matching_math::{flush_level_price, level_fill_atoms};
use dropset_math_core::share::{self, BasketError};

use crate::errors::DropsetError;

use super::{Vault, BPS, FLUSH_BIT, N_LEVELS};

/// Outcome of a `realize_in_place` call — returned so callers can emit
/// a `RealizeEvent` only when `m > 0`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RealizeOutcome {
    /// New shares minted into `leader_shares` (and `total_shares`).
    pub shares_minted: u64,
    /// HWM after this call. Equal to the pre-call value when the vault
    /// is frozen / tombstoned or VPS did not exceed HWM; with no (or
    /// sub-quantum) fee but VPS above HWM it advances to the new VPS
    /// even though `shares_minted == 0`.
    pub hwm_after: u64,
}

/// Per the spec's **Realize**: when `VPS = L / total_shares` exceeds
/// the high-water mark, mint `m` new shares to the leader and bump
/// HWM. Genuine no-op (HWM unchanged, no mint) when:
/// - `total_shares == 0` (vault is unseeded — no shares to dilute);
/// - the geometric-mean liquidity `L = isqrt(base · quote) == 0`
///   (an empty reserve — no value to fee);
/// - `vault.frozen != 0` (per spec, HWM is pinned at freeze time);
/// - `vault.tombstoned != 0` (closed vault; HWM pinned at close time —
///   no perf fee may accrue to a leader who has already exited);
/// - `VPS <= HWM` (no excess to fee).
///
/// At `vault.perf_fee_rate == 0` (or a sub-quantum gain) with
/// `VPS > HWM` no shares are minted, but HWM still advances to VPS so a
/// later fee-rate change can't claw back past historical highs — this
/// is **not** a no-op: the higher HWM is written through to the vault.
///
/// `L`, `HWM`, and the intermediate share math run in `u128` to keep
/// the perf-fee formula precise without overflow on realistic atom
/// scales. The final `m` is clamped back into `u64`.
pub fn realize_in_place(vault: &mut Vault) -> RealizeOutcome {
    let hwm = vault.hwm.get();
    // Frozen / tombstoned vaults pin the HWM at freeze / close time and
    // accrue no perf fee. These read on-chain flags, so the guard stays in
    // the program; the solana-free kernel sees only the scalar share state.
    if vault.frozen.get() || vault.tombstoned.get() {
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: hwm,
        };
    }
    // Pure perf-fee accrual lives in `dropset-math-core` so the off-chain
    // NAV consumers reuse it; the kernel echoes the inputs back unchanged
    // in its no-op branches, so writing the result is always correct.
    let r = share::realize_perf_fee(
        vault.base_atoms.get(),
        vault.quote_atoms.get(),
        vault.total_shares.get(),
        vault.leader_shares.get(),
        hwm,
        vault.perf_fee_rate.get(),
    );
    vault.total_shares = r.total_shares_after.into();
    vault.leader_shares = r.leader_shares_after.into();
    vault.hwm = r.hwm_after.into();
    RealizeOutcome {
        shares_minted: r.shares_minted,
        hwm_after: r.hwm_after,
    }
}

impl Vault {
    /// Materialize [`Vault::remaining`] from the stored
    /// [`LiquidityProfile`](super::LiquidityProfile) + current inventory,
    /// then clear `FLUSH_BIT` from the stamp — the transform the matching
    /// engine arms via `FLUSH_BIT` and the first taker after a
    /// `SetReferencePrice` / `SetLiquidityProfile` runs. All inputs are
    /// vault-local (reference price, quote slot, inventory, profile), so
    /// the flush lives behind this accessor and the matching handler no
    /// longer open-codes the slab-touching loop.
    ///
    /// Per-side `Σ size_bps ≤ BPS` gate, a match-time mirror of the
    /// write-time reject in `set_liquidity_profile` (which rejects
    /// `Σ > BPS` before any `profile` bytes are stored) — both read the
    /// per-side sums from
    /// [`LiquidityProfile::side_size_sums`](super::LiquidityProfile::side_size_sums).
    /// Because no stored profile can currently exceed BPS, this match-time
    /// gate is unreachable defense-in-depth today; it keeps the hot path
    /// robust should that write-time reject ever move here. A side whose
    /// sum exceeds BPS is thrown out of matching — its `remaining` sizes
    /// are written as zero, which the collection loop skips — exactly like
    /// an invalid reference price skips a whole vault, instead of aborting
    /// every taker's swap. This subsumes the per-level case: a single level
    /// `> BPS` forces its side's sum `> BPS`. The stored `profile` bytes
    /// are left intact, so the leader's ladder self-heals the moment they
    /// resubmit a valid one. Writing zeros is not an extra write — the
    /// flush overwrites every `remaining` slot regardless. When a side's
    /// sum holds, every level is `≤ sum ≤ BPS`, so `level_fill_atoms` can
    /// never reject and its `unwrap_or(0)` fallback is unreachable on that
    /// side.
    ///
    /// `#[inline(always)]` so the hot-path codegen is identical to the
    /// former open-coded block — the accessor lift is layering-only and
    /// does not cost the matching loop CU.
    #[inline(always)]
    pub fn materialize_remaining(&mut self) {
        let ref_price = self.reference_price.price;
        let ref_slot = self.reference_price.quote_slot.get();
        let stamp = self.reference_price.stamp.get();
        let base_atoms = self.base_atoms.get();
        let quote_atoms = self.quote_atoms.get();

        let (bid_sum, ask_sum) = self.profile.side_size_sums();
        let bids_ok = bid_sum <= BPS as u32;
        let asks_ok = ask_sum <= BPS as u32;
        for i in 0..N_LEVELS {
            let bid = self.profile.bids[i];
            let ask = self.profile.asks[i];
            self.remaining.bids[i].price =
                flush_level_price(ref_price, bid.price_offset.get(), false);
            self.remaining.bids[i].size = if bids_ok {
                level_fill_atoms(bid.size_bps.get(), quote_atoms).unwrap_or(0)
            } else {
                0
            }
            .into();
            self.remaining.bids[i].expires_at =
                ref_slot.saturating_add(bid.expiry_offset.get()).into();
            self.remaining.asks[i].price =
                flush_level_price(ref_price, ask.price_offset.get(), true);
            self.remaining.asks[i].size = if asks_ok {
                level_fill_atoms(ask.size_bps.get(), base_atoms).unwrap_or(0)
            } else {
                0
            }
            .into();
            self.remaining.asks[i].expires_at =
                ref_slot.saturating_add(ask.expiry_offset.get()).into();
        }
        self.reference_price.stamp = (stamp & !FLUSH_BIT).into();
    }
}

/// Single-leg subsequent-deposit sizing — spec invariant I1
/// (VPS-preserving). Shared by `deposit` (outside) and
/// `deposit_leader`'s non-seeding top-up arm so the rounding direction
/// and slippage semantics stay identical across both; a divergence here
/// is a silent value-leak, not a compile error.
///
/// Exactly one leg is supplied (`base_in XOR quote_in`); the matching
/// leg is derived from the vault's current ratio. Shares are **floored**
/// (`shares_out = leg × total_shares / atoms`) and the basket is rounded
/// **up** (`ceil(shares_out × atoms / total_shares)`) so the vault never
/// under-collects. Both finals are bounded by the caller's `max_*_in`
/// (`BasketSlippage`).
///
/// Returns `(shares_out, base_in_final, quote_in_final)`. Requires
/// `total_shares > 0` — callers reject seeding before this point (the
/// seeding share formula is the `isqrt` basket, not this path).
///
/// Thin wrapper over the solana-free [`share::single_leg_basket`] kernel,
/// mapping its [`BasketError`] back onto the program's `DropsetError` so the
/// on-chain error surface is unchanged.
pub fn single_leg_basket(
    total_shares: u64,
    base_atoms: u64,
    quote_atoms: u64,
    base_in: u64,
    quote_in: u64,
    max_base_in: u64,
    max_quote_in: u64,
) -> Result<(u64, u64, u64)> {
    share::single_leg_basket(
        total_shares,
        base_atoms,
        quote_atoms,
        base_in,
        quote_in,
        max_base_in,
        max_quote_in,
    )
    .map_err(|e| {
        match e {
            BasketError::SingleLegRequired => DropsetError::SingleLegRequired,
            BasketError::MathOverflow => DropsetError::MathOverflow,
            BasketError::BasketSlippage => DropsetError::BasketSlippage,
        }
        .into()
    })
}

/// Apply a deposit's post-transfer inventory + total-share mutation to
/// the vault: add the transferred legs onto `base_atoms` / `quote_atoms`
/// and mint `shares_out` into `total_shares`. Returns
/// `(new_total_shares, new_base_atoms, new_quote_atoms)`.
///
/// Shared by `deposit` (outside) and `deposit_leader` so the inventory /
/// share write block stays byte-identical across both paths — a
/// divergence here is a silent value-leak, not a compile error (the same
/// hazard [`single_leg_basket`] is factored out for). The leg amounts are
/// the seeding / single-leg finals the caller already computed. The
/// leader-only `leader_shares` bump stays in `deposit_leader` — the
/// outside path leaves `leader_shares` untouched — so it is deliberately
/// not part of this shared mutation.
pub fn apply_deposit_inventory(
    vault: &mut Vault,
    base_in_final: u64,
    quote_in_final: u64,
    shares_out: u64,
) -> (u64, u64, u64) {
    let new_base = vault.base_atoms.get() + base_in_final;
    let new_quote = vault.quote_atoms.get() + quote_in_final;
    let new_total = vault.total_shares.get() + shares_out;
    vault.base_atoms = new_base.into();
    vault.quote_atoms = new_quote.into();
    vault.total_shares = new_total.into();
    (new_total, new_base, new_quote)
}

// ── realize_in_place wrapper ─────────────────────────────────────
//
// The pure perf-fee formula (unseeded / VPS-vs-HWM / mint / zero-fee
// scalar cases) is tested in `dropset_math_core::share`. These exercise
// the program's `&mut Vault` wrapper specifically: the on-chain
// `frozen` / `tombstoned` flag guards that short-circuit before the
// kernel, and the write-through of the kernel's result onto the vault.
// They run on a stack-allocated `Vault` — no slab, no AccountBuffer,
// no SVM.
#[cfg(test)]
mod tests {
    use super::super::Q32_32_ONE;
    use super::*;
    use crate::Price;
    use anchor_lang_v2::bytemuck::Zeroable;

    fn seeded_vault(b: u64, q: u64, total: u64, leader: u64, hwm: u64, fee_ppm: u32) -> Vault {
        let mut v = Vault::zeroed();
        v.base_atoms = b.into();
        v.quote_atoms = q.into();
        v.total_shares = total.into();
        v.leader_shares = leader.into();
        v.hwm = hwm.into();
        v.perf_fee_rate = fee_ppm.into();
        v
    }

    #[test]
    fn realize_noop_when_frozen() {
        // Even with VPS above HWM, frozen vaults must not accrue — the
        // guard lives in the wrapper, not the kernel.
        let mut v = seeded_vault(200, 200, 100, 100, Q32_32_ONE, 100_000);
        v.frozen = true.into();
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
    }

    #[test]
    fn realize_noop_when_tombstoned() {
        // A tombstoned vault has exited — no perf fee may accrue to a
        // leader who has already closed, even with VPS above HWM. This
        // guards the `withdraw`-against-tombstone path from minting
        // perf-fee shares to an exited leader.
        let mut v = seeded_vault(200, 200, 100, 100, Q32_32_ONE, 100_000);
        v.tombstoned = true.into();
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
        assert_eq!(v.hwm.get(), Q32_32_ONE, "HWM stays pinned at close time");
    }

    #[test]
    fn realize_writes_kernel_result_through_to_vault() {
        // VPS = 4.0 > HWM 1.0 with a 10% fee mints shares; assert the
        // wrapper writes the kernel's `total_shares` / `leader_shares` /
        // `hwm` back onto the vault.
        let mut v = seeded_vault(400, 400, 100, 100, Q32_32_ONE, 100_000);
        let r = realize_in_place(&mut v);
        assert!(r.shares_minted > 0, "expected perf-fee mint at VPS > HWM");
        // total_shares and leader_shares moved by the same `m`.
        assert_eq!(
            v.total_shares.get() - 100,
            v.leader_shares.get() - 100,
            "perf-fee accrual must move total and leader shares by the same delta"
        );
        // HWM advanced to the post-mint VPS.
        assert_eq!(v.hwm.get(), r.hwm_after);
        assert!(r.hwm_after > Q32_32_ONE);
    }

    // ── materialize_remaining flush wrapper ──────────────────────────
    //
    // The pure per-level pricing / sizing (`flush_level_price`,
    // `level_fill_atoms`) is tested in `dropset_math_core::matching_math`.
    // These exercise the program's `&mut Vault` wrapper specifically: the
    // profile→remaining wiring (correct leg per side, offset direction,
    // absolute expiry), the per-side `Σ size_bps > BPS` zeroing gate, and
    // the `FLUSH_BIT` clear. They run on a stack-allocated `Vault` — no
    // slab, no AccountBuffer, no SVM.

    #[test]
    fn materialize_remaining_wires_profile_and_clears_flush_bit() {
        let reference = Price::from_value(2.0).unwrap();
        let mut v = Vault::zeroed();
        v.reference_price.price = reference;
        v.reference_price.quote_slot = 1_000u32.into();
        // Arm the flush: nonce 5 with FLUSH_BIT set.
        v.reference_price.stamp = (5u64 | FLUSH_BIT).into();
        v.quote_atoms = 2_000_000u64.into();
        v.base_atoms = 1_000_000u64.into();
        // Top-of-book bid (50% of quote) and ask (25% of base).
        v.profile.bids[0].price_offset = 500u32.into();
        v.profile.bids[0].size_bps = 5_000u16.into();
        v.profile.bids[0].expiry_offset = 50u32.into();
        v.profile.asks[0].price_offset = 500u32.into();
        v.profile.asks[0].size_bps = 2_500u16.into();
        v.profile.asks[0].expiry_offset = 60u32.into();

        v.materialize_remaining();

        // FLUSH_BIT cleared, nonce preserved.
        assert_eq!(v.reference_price.stamp.get(), 5);
        // Price materialized off the reference — bids subtract, asks add.
        assert_eq!(
            v.remaining.bids[0].price,
            flush_level_price(reference, 500, false)
        );
        assert_eq!(
            v.remaining.asks[0].price,
            flush_level_price(reference, 500, true)
        );
        // Size is `size_bps` of the matching leg (quote for bids, base for
        // asks).
        assert_eq!(v.remaining.bids[0].size.get(), 1_000_000);
        assert_eq!(v.remaining.asks[0].size.get(), 250_000);
        // Expiry is absolute: `quote_slot + expiry_offset`.
        assert_eq!(v.remaining.bids[0].expires_at.get(), 1_050);
        assert_eq!(v.remaining.asks[0].expires_at.get(), 1_060);
        // Empty levels flush to zero size.
        assert_eq!(v.remaining.bids[1].size.get(), 0);
        assert_eq!(v.remaining.asks[1].size.get(), 0);
    }

    #[test]
    fn materialize_remaining_zeroes_side_over_bps() {
        let mut v = Vault::zeroed();
        v.reference_price.price = Price::from_value(1.0).unwrap();
        v.reference_price.stamp = FLUSH_BIT.into();
        v.quote_atoms = 1_000_000u64.into();
        v.base_atoms = 1_000_000u64.into();
        // Bid side `Σ size_bps = 12_000 > BPS`: the whole side is thrown
        // out of matching (every level size zeroed), leaving the stored
        // profile intact. The ask side is valid and materializes normally.
        v.profile.bids[0].size_bps = 6_000u16.into();
        v.profile.bids[1].size_bps = 6_000u16.into();
        v.profile.asks[0].size_bps = 5_000u16.into();
        assert!(v.profile.side_size_sums().0 > BPS as u32);

        v.materialize_remaining();

        assert_eq!(v.remaining.bids[0].size.get(), 0);
        assert_eq!(v.remaining.bids[1].size.get(), 0);
        assert_eq!(v.remaining.asks[0].size.get(), 500_000);
        // The oversized side's profile bytes are untouched — it self-heals
        // on the leader's next valid write.
        assert_eq!(v.profile.bids[0].size_bps.get(), 6_000);
    }

    #[test]
    fn materialize_remaining_zeroes_ask_side_over_bps() {
        let mut v = Vault::zeroed();
        v.reference_price.price = Price::from_value(1.0).unwrap();
        v.reference_price.stamp = FLUSH_BIT.into();
        v.quote_atoms = 1_000_000u64.into();
        v.base_atoms = 1_000_000u64.into();
        // Mirror of `materialize_remaining_zeroes_side_over_bps`: the ask
        // side is `Σ size_bps = 12_000 > BPS`, so it is thrown out of
        // matching while the valid bid side materializes normally.
        v.profile.asks[0].size_bps = 6_000u16.into();
        v.profile.asks[1].size_bps = 6_000u16.into();
        v.profile.bids[0].size_bps = 5_000u16.into();
        assert!(v.profile.side_size_sums().1 > BPS as u32);

        v.materialize_remaining();

        assert_eq!(v.remaining.asks[0].size.get(), 0);
        assert_eq!(v.remaining.asks[1].size.get(), 0);
        assert_eq!(v.remaining.bids[0].size.get(), 500_000);
        // The oversized side's profile bytes are untouched — it self-heals
        // on the leader's next valid write.
        assert_eq!(v.profile.asks[0].size_bps.get(), 6_000);
    }
}
