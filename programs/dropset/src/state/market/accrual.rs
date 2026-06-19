//! Thin program-side wrappers around the solana-free
//! [`dropset_math_core::share`] kernels: perf-fee realization
//! ([`realize_in_place`], which reads/writes `&mut Vault` state and honors
//! the on-chain `frozen` / `tombstoned` flags) and single-leg
//! subsequent-deposit sizing ([`single_leg_basket`], which maps the
//! kernel's [`BasketError`] back onto [`DropsetError`]). The pure formulas
//! live in `dropset-math-core` so the off-chain NAV consumers reuse them.

use anchor_lang_v2::prelude::*;
use dropset_math_core::share::{self, BasketError};

use crate::errors::DropsetError;

use super::Vault;

/// Outcome of a `realize_in_place` call — returned so callers can emit
/// a `RealizeEvent` only when `m > 0`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RealizeOutcome {
    /// New shares minted into `leader_shares` (and `total_shares`).
    pub shares_minted: u64,
    /// HWM after this call. Equal to the pre-call value when nothing was minted.
    pub hwm_after: u64,
}

/// Per the spec's **Realize**: when `VPS = L / total_shares` exceeds
/// the high-water mark, mint `m` new shares to the leader and bump
/// HWM. No-op when:
/// - `total_shares == 0` (vault is unseeded — no shares to dilute);
/// - `vault.frozen != 0` (per spec, HWM is pinned at freeze time);
/// - `vault.tombstoned != 0` (closed vault; HWM pinned at close time —
///   no perf fee may accrue to a leader who has already exited);
/// - `vault.perf_fee_rate == 0` (no fee to accrue);
/// - `VPS <= HWM` (no excess to fee).
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
}
