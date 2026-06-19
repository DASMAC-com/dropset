//! Bounds-checked access to the [`Vault`] sectors in a [`Market`]'s slab
//! tail. Centralizes the `u32`-index range check so business logic in the
//! instruction handlers stops touching the physical slab layout.

use anchor_lang_v2::prelude::*;

use crate::errors::DropsetError;

use super::{Market, Vault};

/// Typed, bounds-checked access to the [`Vault`] sectors in the slab
/// tail. Centralizes the `u32`-index range check every handler used to
/// open-code as `require!((idx as usize) < len, InvalidSectorIndex)`
/// before indexing `as_slice()` / `as_mut_slice()`, so business logic
/// stops touching the physical slab layout. Both methods are a named
/// borrow over the same `get` / `get_mut` the slice index already
/// performs — zero-cost relative to the previous `[idx]` access, which
/// paid for the identical bounds comparison (it just panicked instead
/// of returning [`DropsetError::InvalidSectorIndex`]).
pub trait VaultAccess {
    /// Borrow sector `sector` immutably, or
    /// [`DropsetError::InvalidSectorIndex`] when it is past the slab
    /// tail.
    fn read_vault(&self, sector: u32) -> Result<&Vault>;

    /// Borrow sector `sector` mutably, or
    /// [`DropsetError::InvalidSectorIndex`] when it is past the slab
    /// tail.
    fn mutate_vault(&mut self, sector: u32) -> Result<&mut Vault>;
}

impl VaultAccess for Market {
    #[inline(always)]
    fn read_vault(&self, sector: u32) -> Result<&Vault> {
        self.as_slice()
            .get(sector as usize)
            .ok_or_else(|| DropsetError::InvalidSectorIndex.into())
    }

    #[inline(always)]
    fn mutate_vault(&mut self, sector: u32) -> Result<&mut Vault> {
        self.as_mut_slice()
            .get_mut(sector as usize)
            .ok_or_else(|| DropsetError::InvalidSectorIndex.into())
    }
}

// ── VaultAccess bounds path ──────────────────────────────────────
//
// The accessor's whole job is to convert the slab's panicking `[idx]`
// index into a graceful `InvalidSectorIndex`. These pin the in-range
// borrow and the one-past-the-end rejection for both the shared and
// exclusive paths.
#[cfg(test)]
mod tests {
    use super::super::test_support::{load_market, setup, SECTORS};
    use super::super::NULL_SECTOR;
    use super::*;

    #[test]
    fn read_vault_returns_in_range_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        // Stamp a sentinel so we can confirm the borrow lands on the
        // requested sector rather than a neighbor.
        market.as_mut_slice()[2].base_atoms = 7777u64.into();
        let v = market.read_vault(2).expect("in-range sector must borrow");
        assert_eq!(v.base_atoms.get(), 7777);
    }

    #[test]
    fn read_vault_rejects_out_of_range_sector() {
        let buf = setup();
        let market = load_market(&buf);
        // `Vault` isn't `Debug`, so unwrap the error via `.err()` rather
        // than `expect_err` (which would format the `Ok` value).
        let expected: ProgramError = DropsetError::InvalidSectorIndex.into();
        // Index `SECTORS` is one past the slab tail.
        assert_eq!(market.read_vault(SECTORS).err().unwrap(), expected);
        // `NULL_SECTOR` is the worst case — must reject, not wrap.
        assert_eq!(market.read_vault(NULL_SECTOR).err().unwrap(), expected);
    }

    #[test]
    fn mutate_vault_returns_in_range_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        market
            .mutate_vault(1)
            .expect("in-range sector must borrow")
            .quote_atoms = 4242u64.into();
        assert_eq!(market.as_slice()[1].quote_atoms.get(), 4242);
    }

    #[test]
    fn mutate_vault_rejects_out_of_range_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        let expected: ProgramError = DropsetError::InvalidSectorIndex.into();
        assert_eq!(market.mutate_vault(SECTORS).err().unwrap(), expected);
    }
}
