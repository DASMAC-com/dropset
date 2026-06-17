//! Pure, consensus-critical share / NAV / PnL accounting kernels.
//!
//! These are the scalar formulas behind the program's deposit, withdraw,
//! and perf-fee accrual paths — the seeding `isqrt`, single-leg deposit
//! sizing, the pro-rata withdrawal slice, the performance-fee mint, the
//! realized-PnL crystallization, and the shares-weighted cost-basis merge.
//! They all **run on-chain** (the program calls them through thin
//! `&mut Vault` / `&mut VaultDepositorHeader` wrappers that read state, call
//! the kernel, and write state back), so a bug here is a consensus bug.
//!
//! They live here, alongside [`crate::price`] and [`crate::matching_math`],
//! so the off-chain consumers — the simulator and the upcoming indexer /
//! order-book views — reuse the exact same arithmetic instead of
//! re-deriving NAV/PnL math that would drift from the engine. Anything typed
//! against the on-chain `Vault` / `VaultDepositorHeader` (Anchor `Pod`
//! wrappers) stays in the program; only the pure scalar formula lives here,
//! keeping this module solana-free and WASM-compatible.

use crate::price::Price;
use crate::PPM;

/// Integer square root via Newton's method, on `u128` to give the
/// matching-engine math headroom. Used for the seeding-deposit share
/// formula `total_shares := isqrt(base × quote)` and for `Realize`
/// (`L = isqrt(base × quote)`).
#[inline]
pub fn isqrt_u128(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    // Initial estimate: half the bit-width of `n` shifted up by one —
    // gives an over-estimate that Newton's method then refines down.
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Why a [`single_leg_basket`] sizing was rejected. Mapped back onto the
/// program's `DropsetError` by the thin wrapper that calls this kernel, so
/// the on-chain error surface is unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BasketError {
    /// Neither or both legs supplied — exactly one is required.
    SingleLegRequired,
    /// Derived `shares_out` is zero or overflows `u64`.
    MathOverflow,
    /// The rounded-up basket exceeds the caller's `max_*_in` bound.
    BasketSlippage,
}

/// Single-leg subsequent-deposit sizing — spec invariant I1
/// (VPS-preserving). Shared by `deposit` (outside) and `deposit_leader`'s
/// non-seeding top-up arm so the rounding direction and slippage semantics
/// stay identical across both; a divergence here is a silent value-leak,
/// not a compile error.
///
/// Exactly one leg is supplied (`base_in XOR quote_in`); the matching
/// leg is derived from the vault's current ratio. Shares are **floored**
/// (`shares_out = leg × total_shares / atoms`) and the basket is rounded
/// **up** (`ceil(shares_out × atoms / total_shares)`) so the vault never
/// under-collects. Both finals are bounded by the caller's `max_*_in`
/// (returning [`BasketError::BasketSlippage`] otherwise).
///
/// Returns `(shares_out, base_in_final, quote_in_final)`. Requires
/// `total_shares > 0` — callers reject seeding before this point (the
/// seeding share formula is the `isqrt` basket, not this path).
#[allow(clippy::too_many_arguments)]
pub fn single_leg_basket(
    total_shares: u64,
    base_atoms: u64,
    quote_atoms: u64,
    base_in: u64,
    quote_in: u64,
    max_base_in: u64,
    max_quote_in: u64,
) -> Result<(u64, u64, u64), BasketError> {
    if (base_in > 0) == (quote_in > 0) {
        return Err(BasketError::SingleLegRequired);
    }
    let ts = total_shares as u128;
    let b = base_atoms as u128;
    let q = quote_atoms as u128;
    let shares_out_u128 = if base_in > 0 {
        ((base_in as u128) * ts) / b
    } else {
        ((quote_in as u128) * ts) / q
    };
    if shares_out_u128 == 0 || shares_out_u128 > u64::MAX as u128 {
        return Err(BasketError::MathOverflow);
    }
    // Basket = ceil(shares_out × leg / total_shares). u128 intermediates;
    // the final values fit in u64 by construction (basket ≤ caller's
    // input + 1).
    let base_in_final = (shares_out_u128 * b).div_ceil(ts);
    let quote_in_final = (shares_out_u128 * q).div_ceil(ts);
    if base_in_final > max_base_in as u128 || quote_in_final > max_quote_in as u128 {
        return Err(BasketError::BasketSlippage);
    }
    Ok((
        shares_out_u128 as u64,
        base_in_final as u64,
        quote_in_final as u64,
    ))
}

/// Floored pro-rata basket slice for a withdrawal of `shares_in` out of
/// `total_shares` against a `(base_atoms, quote_atoms)` inventory:
///
/// ```text
/// slice_base  = floor(shares_in × base_atoms  / total_shares)
/// slice_quote = floor(shares_in × quote_atoms / total_shares)
/// ```
///
/// Rounding **down** keeps the dust in the vault for the benefit of the
/// remaining depositors (spec § Depositor operations → Withdraw). Shared
/// by every withdraw path (`withdraw`, `withdraw_leader`, and both
/// `force_withdraw` arms) so the rounding direction stays identical — a
/// divergence here would be a silent value-leak, not a compile error.
/// Callers apply their own `min_*_out` slippage check on the returned
/// slices.
///
/// `total_shares > 0` is the caller's precondition — every path rejects
/// an empty vault upstream. Each result is bounded by its atom input
/// (`shares_in ≤ total_shares`), so both fit back into `u64`.
pub fn compute_pro_rata_slice(
    shares_in: u64,
    total_shares: u64,
    base_atoms: u64,
    quote_atoms: u64,
) -> (u64, u64) {
    let ts = total_shares as u128;
    let s_in = shares_in as u128;
    let slice_base = (s_in * (base_atoms as u128)) / ts;
    let slice_quote = (s_in * (quote_atoms as u128)) / ts;
    (slice_base as u64, slice_quote as u64)
}

/// New share/HWM state produced by [`realize_perf_fee`]. The wrapper writes
/// these back onto the vault; in the no-op branches every `*_after` equals
/// the corresponding input so the write is a harmless identity.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RealizeResult {
    /// New shares minted into `leader_shares` (and `total_shares`).
    pub shares_minted: u64,
    /// HWM after this call.
    pub hwm_after: u64,
    /// `total_shares` after this call (input + `shares_minted`).
    pub total_shares_after: u64,
    /// `leader_shares` after this call (input + `shares_minted`).
    pub leader_shares_after: u64,
}

/// Performance-fee accrual — the pure scalar core of the program's
/// `realize_in_place`. When `VPS = L / total_shares` exceeds the
/// high-water mark, mint `m` new shares to the leader and bump HWM;
/// otherwise no-op (still trailing HWM up to VPS when there is excess but
/// no fee, so a later fee change can't claw back past historical highs).
///
/// The vault-state guards that gate this in the program — `frozen` /
/// `tombstoned` pin the HWM and skip accrual entirely — read on-chain flags
/// and stay in the wrapper; this kernel sees only the scalar share state.
/// It is a no-op (returns the inputs unchanged) when `total_shares == 0`,
/// `L == 0`, or `VPS <= HWM`.
///
/// `L`, `HWM`, and the intermediate share math run in `u128` to keep the
/// perf-fee formula precise without overflow on realistic atom scales. The
/// final `m` is clamped back into `u64`.
pub fn realize_perf_fee(
    base_atoms: u64,
    quote_atoms: u64,
    total_shares: u64,
    leader_shares: u64,
    hwm: u64,
    perf_fee_rate: u32,
) -> RealizeResult {
    let s = total_shares;
    // No-op result: inputs echoed back unchanged.
    let noop = RealizeResult {
        shares_minted: 0,
        hwm_after: hwm,
        total_shares_after: s,
        leader_shares_after: leader_shares,
    };
    if s == 0 {
        return noop;
    }
    let f_ppm = perf_fee_rate as u128;
    let b = base_atoms as u128;
    let q = quote_atoms as u128;
    let l = isqrt_u128(b.saturating_mul(q));
    if l == 0 {
        return noop;
    }
    // `vps` in Q32.32, same encoding as `hwm`.
    let vps = (l << 32) / (s as u128);
    if vps <= hwm as u128 {
        return noop;
    }
    // Advance HWM up to VPS without minting (no fee, or sub-quantum gain).
    let advanced = RealizeResult {
        hwm_after: vps as u64,
        ..noop
    };
    if f_ppm == 0 {
        // No perf fee — HWM still trails VPS upwards so a later fee
        // change can't claw back past historical highs.
        return advanced;
    }
    // m = f · s · (L − hwm·s) / ((1 − f) · L + f · hwm·s)
    //
    // Working in Q32.32 for the `hwm × s` term, then shifting back
    // before the division so we don't compound the Q32.32 scale across
    // numerator and denominator.
    let s_u = s as u128;
    let hwm_u = hwm as u128;
    let hwm_s = (hwm_u * s_u) >> 32; // back to atom scale
    if l <= hwm_s {
        // VPS rose by sub-quantum (rounding error) — skip.
        return advanced;
    }
    let num = f_ppm * s_u * (l - hwm_s);
    let one_minus_f = PPM as u128 - f_ppm;
    let denom = one_minus_f * l + f_ppm * hwm_s;
    if denom == 0 {
        return noop;
    }
    let m = (num / denom).min(u64::MAX as u128) as u64;
    if m == 0 {
        return advanced;
    }
    let s_after = s.saturating_add(m);
    let leader_after = leader_shares.saturating_add(m);
    let hwm_after = ((l << 32) / s_after as u128) as u64;
    RealizeResult {
        shares_minted: m,
        hwm_after,
        total_shares_after: s_after,
        leader_shares_after: leader_after,
    }
}

/// Why a [`crystallize_pnl`] call failed. Mapped back onto the program's
/// `DropsetError` by the thin wrapper, so the on-chain error surface is
/// unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrystallizeError {
    /// `shares_in` exceeds the position's `shares`.
    InsufficientShares,
    /// An intermediate `u128` product or subtraction overflowed.
    MathOverflow,
}

/// New cost-basis state produced by [`crystallize_pnl`]. The wrapper writes
/// these back onto the depositor header and returns `pnl_delta` for the
/// `WithdrawEvent`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CrystallizeResult {
    /// Signed FX component after this call.
    pub realized_fx: i64,
    /// Signed yield (ex-FX) component after this call.
    pub realized_yield: i64,
    /// Total signed realized PnL after this call.
    pub realized_pnl: i64,
    /// `shares` after burning `shares_in`.
    pub shares_after: u64,
    /// `net_deposits` after releasing the proportional basis slice.
    pub net_deposits_after: u64,
    /// The `realized_pnl` move this call produced (the `WithdrawEvent` value).
    pub pnl_delta: i64,
}

/// Crystallize the realized PnL of withdrawing `shares_in` shares that
/// deliver the `(slice_base, slice_quote)` basket — the pure scalar core of
/// the program's `crystallize_realized_pnl`. Returns the new `realized_*`
/// accumulators, the post-burn `shares` / `net_deposits`, and the
/// `realized_pnl` delta.
///
/// Realized PnL math, per docs/architecture.md
/// § Depositor operations → Withdraw (the realized-PnL formula block):
///   realized_fx    += slice_base × (ref_now − entry_ref)
///   realized_yield += slice_quote + slice_base × entry_ref − released_basis
///   realized_pnl   += slice_quote + slice_base × ref_now    − released_basis
///
/// `ref_now × slice_base` and `entry_ref × slice_base` are decoded via
/// [`Price::quote_for_base`] — both produce a quote-atom value, so the
/// deltas are well-typed in quote-denominated units. All math is in
/// `u128`/`i128` to avoid intermediate overflow; the signed accumulators
/// clamp into `i64`. `entry_vps`, `entry_ref_price`, and `gross_deposited`
/// are intentionally untouched by the caller — a proportional reduction
/// preserves the shares-weighted averages.
#[allow(clippy::too_many_arguments)]
pub fn crystallize_pnl(
    shares_in: u64,
    shares: u64,
    net_deposits: u64,
    slice_base: u64,
    slice_quote: u64,
    entry_ref_price: Price,
    ref_now: Price,
    realized_fx: i64,
    realized_yield: i64,
    realized_pnl: i64,
) -> Result<CrystallizeResult, CrystallizeError> {
    if shares < shares_in {
        return Err(CrystallizeError::InsufficientShares);
    }
    let s_in = shares_in as u128;
    let released_basis = (net_deposits as u128)
        .checked_mul(s_in)
        .ok_or(CrystallizeError::MathOverflow)?
        / (shares as u128);
    let quote_for_ref_now = ref_now.quote_for_base(slice_base).min(i128::MAX as u128) as i128;
    let quote_for_ref_entry = entry_ref_price
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
    let realized_fx = ((realized_fx as i128).saturating_add(fx_delta))
        .clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let new_pnl = (realized_pnl as i128).saturating_add(pnl_delta);
    let new_yield = (realized_yield as i128).saturating_add(yield_delta);
    let realized_pnl = new_pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let realized_yield = new_yield.clamp(i64::MIN as i128, i64::MAX as i128) as i64;

    // Burn the withdrawn shares and reduce the cost basis by the released
    // slice (`net_deposits' = net_deposits − released_basis`).
    let shares_after = shares - shares_in;
    let net_deposits_after = net_deposits
        .checked_sub(released_basis as u64)
        .ok_or(CrystallizeError::MathOverflow)?;

    Ok(CrystallizeResult {
        realized_fx,
        realized_yield,
        realized_pnl,
        shares_after,
        net_deposits_after,
        pnl_delta: pnl_delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64,
    })
}

/// Shares-weighted merge of a depositor's entry basis on a top-off deposit
/// — the pure scalar core of `VaultDepositorHeader::record_deposit`'s
/// top-off arm. Given the prior position (`prior_shares`, `entry_vps_prev`,
/// `entry_ref_prev`) and the incoming lot (`shares_out`, `vps_after`,
/// `ref_now`), returns the merged `(entry_vps_new, entry_ref_new)`.
///
/// `entry_vps` is Q32.32, so a raw u128 weighted average is exact;
/// `entry_ref` routes through [`Price::weighted_average`] (a custom
/// decimal-float that must decode / average / re-encode). See the spec's
/// **Depositor positions and cost basis → Top-off**.
pub fn merge_entry_basis(
    prior_shares: u64,
    shares_out: u64,
    entry_vps_prev: u64,
    vps_after: u64,
    entry_ref_prev: Price,
    ref_now: Price,
) -> (u64, Price) {
    let s = prior_shares as u128;
    let ds = shares_out as u128;
    let denom = s + ds;
    let entry_vps_new = (s * (entry_vps_prev as u128) + ds * (vps_after as u128)) / denom;
    let entry_ref_new = entry_ref_prev.weighted_average(ref_now, s, ds);
    (entry_vps_new as u64, entry_ref_new)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Q32.32 representation of `1.0` — the seeded value-per-share.
    const Q32_32_ONE: u64 = 1u64 << 32;

    // ── compute_pro_rata_slice ──────────────────────────────────────

    #[test]
    fn pro_rata_slice_exact_division() {
        // Half the shares → exactly half of each leg, no remainder.
        assert_eq!(compute_pro_rata_slice(50, 100, 1_000, 2_000), (500, 1_000));
    }

    #[test]
    fn pro_rata_slice_full_withdraw_drains_both_legs() {
        // Burning every share takes the whole inventory.
        assert_eq!(
            compute_pro_rata_slice(100, 100, 1_000, 2_000),
            (1_000, 2_000)
        );
    }

    #[test]
    fn pro_rata_slice_floors_and_leaves_dust() {
        // 1 of 3 shares against 10 atoms → floor(10/3) = 3, leaving the
        // 1-atom remainder in the vault for the other depositors.
        assert_eq!(compute_pro_rata_slice(1, 3, 10, 10), (3, 3));
    }

    #[test]
    fn pro_rata_slice_zero_leg_stays_zero() {
        // A vault holding only quote slices an empty base leg as zero.
        assert_eq!(compute_pro_rata_slice(25, 100, 0, 4_000), (0, 1_000));
    }

    #[test]
    fn pro_rata_slice_no_overflow_at_atom_ceiling() {
        // `shares_in × atoms` is computed in u128, so a u64-ceiling
        // inventory withdrawn in full does not overflow and round-trips
        // back into u64.
        assert_eq!(
            compute_pro_rata_slice(u64::MAX, u64::MAX, u64::MAX, u64::MAX),
            (u64::MAX, u64::MAX)
        );
    }

    // ── single_leg_basket ───────────────────────────────────────────

    #[test]
    fn single_leg_requires_exactly_one_leg() {
        // Neither leg, or both legs, is rejected.
        assert_eq!(
            single_leg_basket(100, 1_000, 1_000, 0, 0, u64::MAX, u64::MAX),
            Err(BasketError::SingleLegRequired)
        );
        assert_eq!(
            single_leg_basket(100, 1_000, 1_000, 10, 10, u64::MAX, u64::MAX),
            Err(BasketError::SingleLegRequired)
        );
    }

    #[test]
    fn single_leg_floors_shares_and_ceils_basket() {
        // 100 base into a 1:1 vault of 1000/1000 with 100 shares:
        // shares_out = floor(100 × 100 / 1000) = 10; basket rounds up to
        // ceil(10 × 1000 / 100) = 100 on each leg.
        assert_eq!(
            single_leg_basket(100, 1_000, 1_000, 100, 0, u64::MAX, u64::MAX),
            Ok((10, 100, 100))
        );
    }

    #[test]
    fn single_leg_rejects_slippage() {
        // The rounded-up basket exceeds a tight max_*_in bound.
        assert_eq!(
            single_leg_basket(100, 1_000, 1_000, 100, 0, 50, u64::MAX),
            Err(BasketError::BasketSlippage)
        );
    }

    #[test]
    fn single_leg_rejects_zero_shares() {
        // A leg too small to buy a whole share floors to zero.
        assert_eq!(
            single_leg_basket(100, 1_000_000, 1_000_000, 1, 0, u64::MAX, u64::MAX),
            Err(BasketError::MathOverflow)
        );
    }

    // ── realize_perf_fee ────────────────────────────────────────────

    #[test]
    fn realize_noop_on_unseeded_vault() {
        let r = realize_perf_fee(0, 0, 0, 0, 0, 100_000);
        assert_eq!(r.shares_minted, 0);
        assert_eq!(r.hwm_after, 0);
        assert_eq!(r.total_shares_after, 0);
    }

    #[test]
    fn realize_noop_when_vps_at_or_below_hwm() {
        // b · q = 10_000 → L = 100, total_shares = 100, VPS = 1.0 = HWM.
        let r = realize_perf_fee(100, 100, 100, 100, Q32_32_ONE, 100_000);
        assert_eq!(r.shares_minted, 0);
        assert_eq!(r.total_shares_after, 100);
        assert_eq!(r.leader_shares_after, 100);
        assert_eq!(r.hwm_after, Q32_32_ONE);
    }

    #[test]
    fn realize_mints_shares_when_vps_exceeds_hwm() {
        // L = isqrt(400 × 400) = 400, total_shares = 100 → VPS = 4.0, so a
        // 10% perf fee mints new shares to the leader.
        let r = realize_perf_fee(400, 400, 100, 100, Q32_32_ONE, 100_000);
        assert!(r.shares_minted > 0, "expected perf-fee mint at VPS > HWM");
        // total and leader shares move by the same delta.
        assert_eq!(
            r.total_shares_after - 100,
            r.leader_shares_after - 100,
            "perf-fee accrual must move total and leader shares by the same delta"
        );
        assert!(r.hwm_after > Q32_32_ONE);
    }

    #[test]
    fn realize_zero_fee_advances_hwm_only() {
        // With perf_fee_rate = 0 the leader earns no shares, but HWM still
        // trails up so a later fee bump cannot retroactively accrue.
        let r = realize_perf_fee(400, 400, 100, 100, Q32_32_ONE, 0);
        assert_eq!(r.shares_minted, 0);
        assert!(r.hwm_after > Q32_32_ONE);
        assert_eq!(r.leader_shares_after, 100);
        assert_eq!(r.total_shares_after, 100);
    }

    // ── crystallize_pnl ─────────────────────────────────────────────

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
        let r = crystallize_pnl(50, 100, 1_000, 100, 0, price_one(), price_two(), 0, 0, 0).unwrap();
        assert_eq!(r.pnl_delta, -300, "returned delta is the realized_pnl move");
        assert_eq!(r.realized_fx, 100);
        assert_eq!(r.realized_yield, -400);
        assert_eq!(r.realized_pnl, -300);
        // Invariant: realized_yield + realized_fx == realized_pnl.
        assert_eq!(r.realized_yield + r.realized_fx, r.realized_pnl);
        // Shares burned and basis reduced by exactly the released slice.
        assert_eq!(r.shares_after, 50);
        assert_eq!(r.net_deposits_after, 500);
    }

    #[test]
    fn crystallize_full_drain_zeroes_shares_and_basis() {
        let r =
            crystallize_pnl(100, 100, 1_000, 0, 1_000, price_one(), price_one(), 0, 0, 0).unwrap();
        assert_eq!(r.shares_after, 0);
        assert_eq!(r.net_deposits_after, 0);
        // Flat reference and slice_quote == basis → no realized move.
        assert_eq!(r.realized_pnl, 0);
    }

    #[test]
    fn crystallize_accumulates_across_calls() {
        // Two successive partial withdrawals at a flat 1.0 reference, then
        // a profitable leg. Each call threads the prior accumulators back
        // in (as the wrapper does by reading then writing the header).
        let r1 =
            crystallize_pnl(25, 100, 1_000, 0, 250, price_one(), price_one(), 0, 0, 0).unwrap();
        assert_eq!(r1.shares_after, 75);
        assert_eq!(r1.net_deposits_after, 750);
        // Reference doubled: an all-base slice of 100 against released
        // basis floor(750 × 25 / 75) = 250 → pnl = 2×100 − 250 = −50.
        let r2 = crystallize_pnl(
            25,
            r1.shares_after,
            r1.net_deposits_after,
            100,
            0,
            price_one(),
            price_two(),
            r1.realized_fx,
            r1.realized_yield,
            r1.realized_pnl,
        )
        .unwrap();
        assert_eq!(r2.pnl_delta, -50);
        assert_eq!(r2.realized_pnl, -50);
        assert_eq!(r2.shares_after, 50);
        assert_eq!(r2.net_deposits_after, 500);
    }

    #[test]
    fn crystallize_rejects_overdraw() {
        assert_eq!(
            crystallize_pnl(20, 10, 100, 0, 0, price_one(), price_one(), 0, 0, 0),
            Err(CrystallizeError::InsufficientShares)
        );
    }

    #[test]
    fn crystallize_saturates_at_i64_ceiling() {
        // A slice_quote near u64::MAX yields a pnl/yield delta far beyond
        // i64::MAX (zero basis, flat reference). The accumulators and the
        // returned delta must saturate at the ceiling, not wrap or panic.
        let r = crystallize_pnl(1, 2, 0, 0, u64::MAX, price_one(), price_one(), 0, 0, 0).unwrap();
        assert_eq!(
            r.pnl_delta,
            i64::MAX,
            "returned delta saturates at i64::MAX"
        );
        assert_eq!(r.realized_pnl, i64::MAX);
        assert_eq!(r.realized_yield, i64::MAX);
        assert_eq!(r.realized_fx, 0);
        // A second positive move stays pinned at the ceiling.
        let r2 = crystallize_pnl(
            1,
            r.shares_after,
            r.net_deposits_after,
            0,
            u64::MAX,
            price_one(),
            price_one(),
            r.realized_fx,
            r.realized_yield,
            r.realized_pnl,
        )
        .unwrap();
        assert_eq!(r2.pnl_delta, i64::MAX);
        assert_eq!(r2.realized_pnl, i64::MAX);
    }

    // ── merge_entry_basis ───────────────────────────────────────────

    #[test]
    fn merge_entry_basis_shares_weighted_average() {
        // Equal-weight top-off: 100 prior shares entered at VPS 1.0 / ref
        // 1.0, 100 fresh shares entering at VPS 2.0 / ref 2.0. The merged
        // entry VPS is the shares-weighted mean (100·1 + 100·2) / 200 = 1.5
        // in Q32.32, and the merged entry ref blends strictly between the
        // two references.
        let (entry_vps, entry_ref) = merge_entry_basis(
            100,
            100,
            Q32_32_ONE,
            2 * Q32_32_ONE,
            price_one(),
            price_two(),
        );
        assert_eq!(entry_vps, Q32_32_ONE + Q32_32_ONE / 2);
        assert!(entry_ref > price_one() && entry_ref < price_two());
    }

    #[test]
    fn merge_entry_basis_weights_by_share_count() {
        // A tiny fresh lot barely moves a large prior position's basis:
        // 999 prior shares at VPS 1.0, 1 fresh share at VPS 2.0 →
        // (999·1 + 1·2) / 1000 = 1.001, still essentially 1.0.
        let (entry_vps, _) =
            merge_entry_basis(999, 1, Q32_32_ONE, 2 * Q32_32_ONE, price_one(), price_two());
        // (999 + 2) / 1000 of Q32_32_ONE, floored.
        let expected = (999 * (Q32_32_ONE as u128) + 2 * (Q32_32_ONE as u128)) / 1000;
        assert_eq!(entry_vps, expected as u64);
    }
}
