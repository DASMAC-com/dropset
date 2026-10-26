//! The deposit's and withdraw's inventory mutations, and the resting-size
//! rescale both apply. They wrap no `dropset-math-core` kernel, so they
//! live apart from the kernel wrappers in `accrual`.

use anchor_lang::prelude::*;

use crate::errors::DropsetError;

use super::{Vault, N_LEVELS};

/// Rescale the vault's materialized resting sizes onto new inventory, leg
/// by leg: each counter is multiplied by `new_leg / old_leg` of the leg it
/// is denominated in (floored), so every level keeps the fraction of its
/// leg it held before the move. Call it before writing the new legs; it
/// reads the old ones off the vault.
///
/// Sizes are materialized once, at flush, as `size_bps × leg_atoms`, and
/// the swap caps a fill only at live inventory. Without this, a withdrawal
/// leaves each level sized against the pre-withdraw leg — a 20% level can
/// become the whole of what remains, and a taker empties the leg at
/// top-of-book instead of walking the ladder.
///
/// Re-arming `FLUSH_BIT` instead would be wrong: every flush zeroes
/// `flip_size`, so any outside depositor could wipe the leader's flipped
/// proceeds with a small deposit and withdraw. `scale_flips` is `true` on
/// withdraw, which removes the inventory backing flipped allowance too, and
/// `false` on deposit, where inflating `flip_size` would mint flipped
/// allowance no fill backs and break the equal-atoms flip ledger.
pub fn rescale_remaining(vault: &mut Vault, new_base: u64, new_quote: u64, scale_flips: bool) {
    let old_base = vault.base_atoms.get();
    let old_quote = vault.quote_atoms.get();
    // Asks rest base and flip into quote; bids rest quote and flip into base.
    for i in 0..N_LEVELS {
        let ask = &mut vault.remaining.asks[i];
        ask.size = scale_atoms(ask.size.get(), old_base, new_base).into();
        if scale_flips {
            ask.flip_size = scale_atoms(ask.flip_size.get(), old_quote, new_quote).into();
        }
        let bid = &mut vault.remaining.bids[i];
        bid.size = scale_atoms(bid.size.get(), old_quote, new_quote).into();
        if scale_flips {
            bid.flip_size = scale_atoms(bid.flip_size.get(), old_base, new_base).into();
        }
    }
}

/// `atoms × new_leg / old_leg`, floored and clamped to `u64`. An unchanged
/// or previously empty leg leaves `atoms` as is.
#[inline(always)]
fn scale_atoms(atoms: u64, old_leg: u64, new_leg: u64) -> u64 {
    if atoms == 0 || old_leg == new_leg || old_leg == 0 {
        return atoms;
    }
    let scaled = (atoms as u128) * (new_leg as u128) / (old_leg as u128);
    scaled.min(u64::MAX as u128) as u64
}

/// Apply a deposit's post-transfer inventory + total-share mutation to
/// the vault: add the transferred legs onto `base_atoms` / `quote_atoms`
/// and mint `shares_out` into `total_shares`. Returns
/// `(new_total_shares, new_base_atoms, new_quote_atoms)`.
///
/// Every add is checked and fails with `MathOverflow` rather than
/// aborting; nothing is written unless all three fit. The leg transfers
/// run before this call, so an overflow here reverts them with the rest
/// of the transaction.
///
/// Shared by `deposit` (outside) and `deposit_leader` so the inventory /
/// share write block stays byte-identical across both paths — a
/// divergence here is a silent value-leak, not a compile error (the same
/// hazard `single_leg_basket` is factored out for). The leg amounts are
/// the seeding / single-leg finals the caller already computed. The
/// leader-only `leader_shares` bump stays in `deposit_leader` — the
/// outside path leaves `leader_shares` untouched — so it is deliberately
/// not part of this shared mutation.
///
/// Resting sizes are scaled up onto the new legs (`flip_size` is left
/// alone; see [`rescale_remaining`]).
pub fn apply_deposit_inventory(
    vault: &mut Vault,
    base_in_final: u64,
    quote_in_final: u64,
    shares_out: u64,
) -> Result<(u64, u64, u64)> {
    let new_base = vault
        .base_atoms
        .get()
        .checked_add(base_in_final)
        .ok_or(DropsetError::MathOverflow)?;
    let new_quote = vault
        .quote_atoms
        .get()
        .checked_add(quote_in_final)
        .ok_or(DropsetError::MathOverflow)?;
    let new_total = vault
        .total_shares
        .get()
        .checked_add(shares_out)
        .ok_or(DropsetError::MathOverflow)?;
    rescale_remaining(vault, new_base, new_quote, false);
    vault.base_atoms = new_base.into();
    vault.quote_atoms = new_quote.into();
    vault.total_shares = new_total.into();
    Ok((new_total, new_base, new_quote))
}

/// Apply a withdrawal's inventory + total-share burn to the vault: take
/// the floored pro-rata slice off `base_atoms` / `quote_atoms`, burn
/// `shares_in` from `total_shares`, and rescale the resting sizes and
/// flipped allowances onto what remains ([`rescale_remaining`]). Returns
/// `(new_total_shares, new_base_atoms, new_quote_atoms)`.
///
/// Shared by all four withdraw paths (`withdraw`, `withdraw_leader` and
/// both `force_withdraw` arms) for the same reason
/// [`apply_deposit_inventory`] is. The subtractions are unchecked by
/// invariant: `shares_in ≤ total_shares`, and the slice is a floored
/// fraction of each leg. The `leader_shares` burn stays with the leader
/// paths.
pub fn apply_withdraw_inventory(
    vault: &mut Vault,
    slice_base: u64,
    slice_quote: u64,
    shares_in: u64,
) -> (u64, u64, u64) {
    let new_total = vault.total_shares.get() - shares_in;
    let new_base = vault.base_atoms.get() - slice_base;
    let new_quote = vault.quote_atoms.get() - slice_quote;
    rescale_remaining(vault, new_base, new_quote, true);
    vault.total_shares = new_total.into();
    vault.base_atoms = new_base.into();
    vault.quote_atoms = new_quote.into();
    (new_total, new_base, new_quote)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang::bytemuck::Zeroable;

    #[test]
    fn apply_deposit_inventory_adds_legs_and_shares() {
        let mut v = Vault::zeroed();
        v.base_atoms = 10u64.into();
        v.quote_atoms = 20u64.into();
        v.total_shares = 5u64.into();
        assert_eq!(
            apply_deposit_inventory(&mut v, 1, 2, 3).unwrap(),
            (8, 11, 22)
        );
        assert_eq!(v.total_shares.get(), 8);
    }

    #[test]
    fn apply_deposit_inventory_overflow_is_typed_and_writes_nothing() {
        // A near-ceiling share count: the share add overflows. It must fail typed, leaving the vault untouched.
        let mut v = Vault::zeroed();
        v.base_atoms = 2u64.into();
        v.quote_atoms = 1u64.into();
        v.total_shares = (u64::MAX - 1).into();
        assert!(apply_deposit_inventory(&mut v, 1, 1, 2).is_err());
        assert_eq!(v.base_atoms.get(), 2);
        assert_eq!(v.quote_atoms.get(), 1);
        assert_eq!(v.total_shares.get(), u64::MAX - 1);
    }

    /// A vault at (1000 base, 4000 quote) with level 0 resting 200 base on
    /// the ask, 800 quote on the bid, and flipped allowance on both.
    fn flushed_vault() -> Vault {
        let mut v = Vault::zeroed();
        v.base_atoms = 1_000u64.into();
        v.quote_atoms = 4_000u64.into();
        v.total_shares = 100u64.into();
        v.remaining.asks[0].size = 200u64.into();
        v.remaining.asks[0].flip_size = 40u64.into();
        v.remaining.bids[0].size = 800u64.into();
        v.remaining.bids[0].flip_size = 10u64.into();
        v
    }

    #[test]
    fn apply_withdraw_inventory_scales_sizes_and_flips_per_leg() {
        let mut v = flushed_vault();
        // Burn half the shares: half of each leg leaves.
        assert_eq!(
            apply_withdraw_inventory(&mut v, 500, 2_000, 50),
            (50, 500, 2_000)
        );
        assert_eq!(v.remaining.asks[0].size.get(), 100);
        assert_eq!(v.remaining.asks[0].flip_size.get(), 20);
        assert_eq!(v.remaining.bids[0].size.get(), 400);
        assert_eq!(v.remaining.bids[0].flip_size.get(), 5);
    }

    #[test]
    fn apply_withdraw_inventory_floors_and_drains_to_zero() {
        let mut v = flushed_vault();
        // 1/3 of the base leg leaves: 200 × 667 / 1000 floors to 133.
        apply_withdraw_inventory(&mut v, 333, 0, 0);
        assert_eq!(v.remaining.asks[0].size.get(), 133);
        // The quote leg is untouched, so its counters are too.
        assert_eq!(v.remaining.bids[0].size.get(), 800);
        assert_eq!(v.remaining.asks[0].flip_size.get(), 40);
        // A full drain zeroes everything resting.
        let mut v = flushed_vault();
        apply_withdraw_inventory(&mut v, 1_000, 4_000, 100);
        assert_eq!(v.remaining.asks[0].size.get(), 0);
        assert_eq!(v.remaining.bids[0].flip_size.get(), 0);
    }

    #[test]
    fn apply_deposit_inventory_scales_sizes_but_not_flips() {
        let mut v = flushed_vault();
        apply_deposit_inventory(&mut v, 1_000, 4_000, 100).unwrap();
        assert_eq!(v.remaining.asks[0].size.get(), 400);
        assert_eq!(v.remaining.bids[0].size.get(), 1_600);
        assert_eq!(v.remaining.asks[0].flip_size.get(), 40);
        assert_eq!(v.remaining.bids[0].flip_size.get(), 10);
    }

    #[test]
    fn rescale_remaining_leaves_an_empty_leg_alone() {
        // A seeding deposit onto an empty vault has nothing to scale from.
        let mut v = Vault::zeroed();
        v.remaining.asks[0].size = 7u64.into();
        rescale_remaining(&mut v, 10, 10, false);
        assert_eq!(v.remaining.asks[0].size.get(), 7);
    }
}
