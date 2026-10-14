//! The deposit's post-transfer inventory mutation. It wraps no
//! `dropset-math-core` kernel, so it lives apart from the kernel wrappers
//! in `accrual`.

use anchor_lang_v2::prelude::*;

use crate::errors::DropsetError;

use super::Vault;

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
    vault.base_atoms = new_base.into();
    vault.quote_atoms = new_quote.into();
    vault.total_shares = new_total.into();
    Ok((new_total, new_base, new_quote))
}

#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang_v2::bytemuck::Zeroable;

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
        // A single-atom vault with a near-ceiling share count: the share
        // add overflows. It must fail typed, leaving the vault untouched.
        let mut v = Vault::zeroed();
        v.base_atoms = 2u64.into();
        v.quote_atoms = 1u64.into();
        v.total_shares = (u64::MAX - 1).into();
        assert!(apply_deposit_inventory(&mut v, 1, 1, 2).is_err());
        assert_eq!(v.base_atoms.get(), 2);
        assert_eq!(v.quote_atoms.get(), 1);
        assert_eq!(v.total_shares.get(), u64::MAX - 1);
    }
}
