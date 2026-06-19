//! Doubly-linked-list surgery over the [`Vault`] sectors in a [`Market`]'s
//! slab tail. The three list heads ([`MarketHeader::head`],
//! [`MarketHeader::tombstone_head`], [`MarketHeader::free_head`]) are
//! mutated only here, through [`VaultDll`]; [`DllList`] names which head a
//! given operation targets.

use anchor_lang_v2::{bytemuck::Zeroable, prelude::*};

use crate::errors::DropsetError;

use super::{Market, MarketHeader, Vault, NULL_SECTOR};

/// Doubly-linked-list operations over the [`Vault`] sectors threaded by
/// `next` / `prev`. The three list heads
/// ([`MarketHeader::head`], [`MarketHeader::tombstone_head`],
/// [`MarketHeader::free_head`]) are mutated only here.
///
/// Sector indices are `u32` positions into the slab tail; [`NULL_SECTOR`]
/// marks both ends of a list. The list heads form an enum-like trio
/// represented by [`DllList`] so call sites can avoid a `&mut PodU32`
/// borrow into the header through the slab's `DerefMut`.
pub trait VaultDll {
    /// Allocate a sector: prefers the free list, else extends the slab
    /// by one entry. Returns the new sector's index; sets `leader`,
    /// `quote_authority`, and bookkeeping fields to default but does
    /// **not** thread it onto the active list — the caller does that
    /// after stamping the rest of the vault.
    fn allocate_sector(&mut self, payer: &AccountView) -> Result<u32>;

    /// Push `sector` to the front of `list`. Updates list head, links
    /// `sector.next` to the old head, links the old head's `prev` to
    /// `sector` (when non-null), and sets `sector.prev` to
    /// [`NULL_SECTOR`].
    fn link_head(&mut self, list: DllList, sector: u32) -> Result<()>;

    /// Unlink `sector` from `list`. Patches the neighbors' pointers
    /// and the list head; leaves `sector.next` / `sector.prev` as
    /// `NULL_SECTOR` afterwards.
    fn unlink(&mut self, list: DllList, sector: u32) -> Result<()>;

    /// Locate which threaded list (active or tombstone) currently holds
    /// `sector` by walking each head. Returns `None` when the sector is
    /// on neither — i.e. it is on the free list or otherwise detached.
    /// The free list is intentionally excluded: callers reclaiming a
    /// drained sector want to know whether it was active (so they can
    /// decrement `active_count`) or tombstoned, and a free sector is
    /// already where reclaim would put it. Bounded by
    /// `registry.max_vaults_per_market` (a `u8`), so the walk is cheap.
    fn vault_list_of(&self, sector: u32) -> Option<DllList>;

    /// Reclaim a fully-drained sector (`total_shares == 0`) to the free
    /// DLL. Finds the sector's current list, unlinks it (decrementing
    /// `active_count` when it was active), zeroes `leader` so the
    /// emptiness marker holds while it sits on the free list, and
    /// prepends it to `free_head`. The vault's inventory / share
    /// counters are the caller's responsibility — this is purely the
    /// list-pointer move the spec's **Vault → Frozen and tombstoned
    /// vaults → Reclaim** step describes (no rent is refunded; sectors
    /// are inline in the market slab).
    fn reclaim_sector(&mut self, sector: u32) -> Result<()>;
}

/// Identifies which of the three DLL heads on the [`MarketHeader`] a
/// given operation targets.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DllList {
    /// Active vaults — visited by the matching engine.
    Active,
    /// Tombstoned vaults — closed by their leader, still holding shares.
    Tombstone,
    /// Free sectors — available for reuse.
    Free,
}

impl DllList {
    /// Read the corresponding list head out of the header.
    #[inline(always)]
    fn head(self, h: &MarketHeader) -> u32 {
        match self {
            DllList::Active => h.head.get(),
            DllList::Tombstone => h.tombstone_head.get(),
            DllList::Free => h.free_head.get(),
        }
    }

    /// Write the corresponding list head into the header.
    #[inline(always)]
    fn set_head(self, h: &mut MarketHeader, value: u32) {
        match self {
            DllList::Active => h.head = value.into(),
            DllList::Tombstone => h.tombstone_head = value.into(),
            DllList::Free => h.free_head = value.into(),
        }
    }
}

impl VaultDll for Market {
    fn allocate_sector(&mut self, payer: &AccountView) -> Result<u32> {
        // Free list first — reuse a reclaimed sector if any.
        let free_head = self.free_head.get();
        if free_head != NULL_SECTOR {
            let next = self.as_slice()[free_head as usize].next.get();
            self.free_head = next.into();
            // Re-initialize the sector. Zero the whole vault so previous
            // state doesn't leak; the caller stamps the new fields.
            self.as_mut_slice()[free_head as usize] = Vault::zeroed();
            self.as_mut_slice()[free_head as usize].next = NULL_SECTOR.into();
            self.as_mut_slice()[free_head as usize].prev = NULL_SECTOR.into();
            return Ok(free_head);
        }
        // Else grow the tail by one. Rejects with `AccountDataTooSmall`
        // if `try_push` finds no room, which our `resize_to_capacity`
        // call rules out unless the realloc itself is rejected.
        let new_len = self.len() as u32;
        require!(new_len < u32::MAX, ProgramError::ArithmeticOverflow);
        self.resize_to_capacity(new_len + 1)?;
        self.top_up(payer)?;
        let mut sector = Vault::zeroed();
        sector.next = NULL_SECTOR.into();
        sector.prev = NULL_SECTOR.into();
        self.try_push(sector)?;
        Ok(new_len)
    }

    fn link_head(&mut self, list: DllList, sector: u32) -> Result<()> {
        let len = self.len();
        require!((sector as usize) < len, DropsetError::InvalidSectorIndex);
        // The sector must be detached. `allocate_sector` and `unlink`
        // both leave a sector with `(next, prev) = (NULL, NULL)`;
        // anything else here is a double-link that would silently
        // corrupt whichever list it currently sits on.
        {
            let v = &self.as_slice()[sector as usize];
            require!(
                v.next.get() == NULL_SECTOR && v.prev.get() == NULL_SECTOR,
                DropsetError::CorruptVaultList
            );
        }
        let prev_head = list.head(self);
        // Stamp the new sector's pointers.
        {
            let v = &mut self.as_mut_slice()[sector as usize];
            v.next = prev_head.into();
            v.prev = NULL_SECTOR.into();
        }
        // Patch the old head's `prev`, if any.
        if prev_head != NULL_SECTOR {
            require!((prev_head as usize) < len, DropsetError::CorruptVaultList);
            self.as_mut_slice()[prev_head as usize].prev = sector.into();
        }
        list.set_head(self, sector);
        Ok(())
    }

    fn unlink(&mut self, list: DllList, sector: u32) -> Result<()> {
        let len = self.len();
        require!((sector as usize) < len, DropsetError::InvalidSectorIndex);
        let (next, prev) = {
            let v = &self.as_slice()[sector as usize];
            (v.next.get(), v.prev.get())
        };
        // Patch the neighbors.
        if prev != NULL_SECTOR {
            require!((prev as usize) < len, DropsetError::CorruptVaultList);
            self.as_mut_slice()[prev as usize].next = next.into();
        } else {
            // No `prev` — this sector was the list head. Move it on.
            require!(list.head(self) == sector, DropsetError::CorruptVaultList);
            list.set_head(self, next);
        }
        if next != NULL_SECTOR {
            require!((next as usize) < len, DropsetError::CorruptVaultList);
            self.as_mut_slice()[next as usize].prev = prev.into();
        }
        // Detach the unlinked sector — leaves a known-clean state for
        // the caller to re-thread onto another list.
        let v = &mut self.as_mut_slice()[sector as usize];
        v.next = NULL_SECTOR.into();
        v.prev = NULL_SECTOR.into();
        Ok(())
    }

    fn vault_list_of(&self, sector: u32) -> Option<DllList> {
        for list in [DllList::Active, DllList::Tombstone] {
            let mut cur = list.head(self);
            while cur != NULL_SECTOR {
                if cur == sector {
                    return Some(list);
                }
                cur = self.as_slice()[cur as usize].next.get();
            }
        }
        None
    }

    fn reclaim_sector(&mut self, sector: u32) -> Result<()> {
        let len = self.len();
        require!((sector as usize) < len, DropsetError::InvalidSectorIndex);
        // The sector must currently be threaded on active or tombstone.
        // A sector that's already free (or otherwise unreachable) is a
        // double-reclaim and would corrupt the free list.
        let list = self
            .vault_list_of(sector)
            .ok_or(DropsetError::CorruptVaultList)?;
        self.unlink(list, sector)?;
        // Only the active list is counted; tombstoned sectors were
        // already removed from `active_count` when they were closed.
        if list == DllList::Active {
            let prev = self.active_count.get();
            self.active_count = prev.saturating_sub(1).into();
        }
        // Zero the emptiness marker so `leader == default` holds for as
        // long as the sector sits on the free list — `allocate_sector`
        // re-zeroes the whole struct on reuse, but keeping the invariant
        // true in the interim matches what every other reader expects.
        self.as_mut_slice()[sector as usize].leader = Address::default();
        self.link_head(DllList::Free, sector)?;
        Ok(())
    }
}

/// Unit tests driving the **real** `Market` slab via a stack-backed
/// `AccountBuffer` from `anchor_lang_v2::testing`. Pre-allocates a
/// fixed number of sectors up front (see [`super::test_support`]) and
/// exercises the pointer-arithmetic surface (`link_head`, `unlink`,
/// free-list reuse of `allocate_sector`) directly against the on-chain
/// code path.
///
/// `allocate_sector`'s tail-growth branch calls `resize_to_capacity` +
/// `top_up` (a system-program transfer CPI), neither of which has a
/// host-side mock in this scaffold. That branch is covered by the
/// `CreateVault` integration tests in a downstream PR — every other
/// `VaultDll` line lives here.
#[cfg(test)]
mod tests {
    use super::super::test_support::{load_market, setup, SECTORS};
    use super::*;
    use anchor_lang_v2::testing::AccountBuffer;

    /// Walk `list` starting at its head and return the visit order.
    /// Used by tests to assert ordering without baking it into the
    /// `VaultDll` trait (the production matching engine iterates
    /// inline, not via a helper).
    fn walk(market: &Market, list: DllList) -> std::vec::Vec<u32> {
        let mut out = std::vec::Vec::new();
        let mut cur = list.head(market);
        while cur != NULL_SECTOR {
            out.push(cur);
            cur = market.as_slice()[cur as usize].next.get();
        }
        out
    }

    #[test]
    fn empty_lists_walk_to_nothing() {
        let buf = setup();
        let market = load_market(&buf);
        assert!(walk(&market, DllList::Active).is_empty());
        assert!(walk(&market, DllList::Tombstone).is_empty());
        assert!(walk(&market, DllList::Free).is_empty());
    }

    #[test]
    fn link_head_prepends_most_recent() {
        let buf = setup();
        let mut market = load_market(&buf);
        market.link_head(DllList::Active, 0).unwrap();
        market.link_head(DllList::Active, 1).unwrap();
        market.link_head(DllList::Active, 2).unwrap();
        // Spec: "New vaults are prepended at `head`" — most recent first.
        assert_eq!(walk(&market, DllList::Active), [2, 1, 0]);
    }

    #[test]
    fn link_head_rejects_out_of_range_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        // The slab tail holds `SECTORS` entries; index `SECTORS` is
        // one past the end and must be rejected before any pointer
        // is patched.
        let err = market
            .link_head(DllList::Active, SECTORS)
            .expect_err("sector >= len must be rejected");
        // The mapped ProgramError comes from DropsetError::InvalidSectorIndex.
        let expected: ProgramError = DropsetError::InvalidSectorIndex.into();
        assert_eq!(err, expected);
        // List head was not mutated.
        assert_eq!(market.head.get(), NULL_SECTOR);
    }

    #[test]
    fn link_head_rejects_already_linked_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        // Link sector 0 onto Active. Its `next` now points at the
        // old head (NULL_SECTOR), which is fine; but a stale linked
        // sector with a non-NULL `prev` (from somewhere else)
        // would silently corrupt the destination list on re-link.
        // Simulate that by stamping `prev = 1` on a fresh sector and
        // attempt to link it.
        market.as_mut_slice()[1].prev = 0u32.into();
        let err = market
            .link_head(DllList::Active, 1)
            .expect_err("double-link of an already-linked sector must be rejected");
        let expected: ProgramError = DropsetError::CorruptVaultList.into();
        assert_eq!(err, expected);
        // Nothing was mutated downstream of the guard.
        assert_eq!(market.head.get(), NULL_SECTOR);
        assert_eq!(market.as_slice()[1].next.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_middle_patches_neighbors() {
        let buf = setup();
        let mut market = load_market(&buf);
        for s in 0..3 {
            market.link_head(DllList::Active, s).unwrap();
        }
        // Order: 2 <-> 1 <-> 0. Unlink the middle.
        market.unlink(DllList::Active, 1).unwrap();
        assert_eq!(walk(&market, DllList::Active), [2, 0]);
        assert_eq!(market.as_slice()[2].next.get(), 0);
        assert_eq!(market.as_slice()[0].prev.get(), 2);
        // The detached sector is left with NULL pointers so it's
        // safe to re-thread onto another list.
        assert_eq!(market.as_slice()[1].next.get(), NULL_SECTOR);
        assert_eq!(market.as_slice()[1].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_head_promotes_next() {
        let buf = setup();
        let mut market = load_market(&buf);
        for s in 0..3 {
            market.link_head(DllList::Active, s).unwrap();
        }
        // Order: 2 <-> 1 <-> 0. Unlink the head (sector 2).
        market.unlink(DllList::Active, 2).unwrap();
        assert_eq!(walk(&market, DllList::Active), [1, 0]);
        assert_eq!(market.head.get(), 1);
        assert_eq!(market.as_slice()[1].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_tail_leaves_predecessor_at_end() {
        let buf = setup();
        let mut market = load_market(&buf);
        for s in 0..3 {
            market.link_head(DllList::Active, s).unwrap();
        }
        // Order: 2 <-> 1 <-> 0. Unlink the tail (sector 0).
        market.unlink(DllList::Active, 0).unwrap();
        assert_eq!(walk(&market, DllList::Active), [2, 1]);
        assert_eq!(market.as_slice()[1].next.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_only_element_empties_list() {
        let buf = setup();
        let mut market = load_market(&buf);
        market.link_head(DllList::Active, 0).unwrap();
        market.unlink(DllList::Active, 0).unwrap();
        assert!(walk(&market, DllList::Active).is_empty());
        assert_eq!(market.head.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_rejects_orphan_head() {
        // Manually corrupt the slab so a sector's `prev == NULL` but
        // the list head doesn't point at it — that's the inconsistent
        // "head-or-orphan" branch of `unlink`'s defensive check
        // (line ~341). Without the require! this would happily set
        // `head = next` based on the orphan's pointers and corrupt
        // the visible list. The Active list is left empty (head =
        // NULL_SECTOR) while sector 0 carries `prev = NULL` from
        // setup but isn't reachable from any head.
        let buf = setup();
        let mut market = load_market(&buf);
        // sector 0 already has prev=NULL from setup; market.head is
        // also NULL. The orphan case fires because sector 0 is not
        // the active head.
        let err = market
            .unlink(DllList::Active, 0)
            .expect_err("unlink of an orphan that isn't the head must be rejected");
        let expected: ProgramError = DropsetError::CorruptVaultList.into();
        assert_eq!(err, expected);
    }

    #[test]
    fn unlink_rejects_out_of_range_sector() {
        let buf = setup();
        let mut market = load_market(&buf);
        let err = market
            .unlink(DllList::Active, SECTORS)
            .expect_err("sector >= len must be rejected");
        let expected: ProgramError = DropsetError::InvalidSectorIndex.into();
        assert_eq!(err, expected);
    }

    #[test]
    fn active_to_tombstone_moves_independently() {
        let buf = setup();
        let mut market = load_market(&buf);
        market.link_head(DllList::Active, 0).unwrap();
        market.link_head(DllList::Active, 1).unwrap();
        // CloseVault flow: unlink from active, prepend to tombstone.
        market.unlink(DllList::Active, 0).unwrap();
        market.link_head(DllList::Tombstone, 0).unwrap();
        assert_eq!(walk(&market, DllList::Active), [1]);
        assert_eq!(walk(&market, DllList::Tombstone), [0]);
        // The two lists must not share members or pointers.
        assert_eq!(market.as_slice()[0].next.get(), NULL_SECTOR);
        assert_eq!(market.as_slice()[1].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn allocate_sector_recycles_from_free_list() {
        let buf = setup();
        let mut market = load_market(&buf);
        // Push two sectors onto the free list — LIFO order, so the
        // next two `allocate_sector` calls pop sector 1 then sector 0.
        market.link_head(DllList::Free, 0).unwrap();
        market.link_head(DllList::Free, 1).unwrap();
        // Scribble on sector 1 to verify allocate re-zeroes it.
        market.as_mut_slice()[1].base_atoms = 12345u64.into();
        market.as_mut_slice()[1].leader_shares = 999u64.into();

        // Dummy payer — `allocate_sector`'s free-list branch never
        // reads it. Doesn't have to be funded.
        let payer_buf = AccountBuffer::<128>::new();
        payer_buf.init([0xBB; 32], [0u8; 32], 0, true, true, false);
        let payer = unsafe { payer_buf.view() };

        let reused = market.allocate_sector(&payer).unwrap();
        assert_eq!(reused, 1);
        // Free head moved on to sector 0.
        assert_eq!(market.free_head.get(), 0);
        // Re-zeroed.
        assert_eq!(market.as_slice()[1].base_atoms.get(), 0);
        assert_eq!(market.as_slice()[1].leader_shares.get(), 0);
        assert_eq!(market.as_slice()[1].next.get(), NULL_SECTOR);
        assert_eq!(market.as_slice()[1].prev.get(), NULL_SECTOR);

        // Second allocate drains the free list.
        let reused2 = market.allocate_sector(&payer).unwrap();
        assert_eq!(reused2, 0);
        assert_eq!(market.free_head.get(), NULL_SECTOR);
    }

    #[test]
    fn free_list_lifo_order() {
        let buf = setup();
        let mut market = load_market(&buf);
        // Push 0, 1, 2 onto the free list. LIFO: 2, 1, 0.
        market.link_head(DllList::Free, 0).unwrap();
        market.link_head(DllList::Free, 1).unwrap();
        market.link_head(DllList::Free, 2).unwrap();

        let payer_buf = AccountBuffer::<128>::new();
        payer_buf.init([0xBB; 32], [0u8; 32], 0, true, true, false);
        let payer = unsafe { payer_buf.view() };

        assert_eq!(market.allocate_sector(&payer).unwrap(), 2);
        assert_eq!(market.allocate_sector(&payer).unwrap(), 1);
        assert_eq!(market.allocate_sector(&payer).unwrap(), 0);
        assert_eq!(market.free_head.get(), NULL_SECTOR);
    }

    #[test]
    fn reclaim_then_reopen_cross_list_lifecycle() {
        // End-to-end exercise of the sector lifecycle the spec
        // describes: an active vault is reclaimed (active → free),
        // a subsequent `CreateVault` reuses it (free → active), and
        // the second occupant's view of the active DLL is consistent
        // with the first occupant having gone through the free list.
        let buf = setup();
        let mut market = load_market(&buf);
        let payer_buf = AccountBuffer::<128>::new();
        payer_buf.init([0xBB; 32], [0u8; 32], 0, true, true, false);
        let payer = unsafe { payer_buf.view() };

        // Sector 0 lives on Active.
        market.link_head(DllList::Active, 0).unwrap();
        assert_eq!(walk(&market, DllList::Active), [0]);

        // Reclaim: unlink from Active, push onto Free.
        market.unlink(DllList::Active, 0).unwrap();
        market.link_head(DllList::Free, 0).unwrap();
        assert!(walk(&market, DllList::Active).is_empty());
        assert_eq!(walk(&market, DllList::Free), [0]);

        // Reopen: allocate_sector pops the free list, returns the
        // reclaimed index, and the caller links it back onto Active.
        let reused = market.allocate_sector(&payer).unwrap();
        assert_eq!(reused, 0);
        assert_eq!(market.free_head.get(), NULL_SECTOR);
        market.link_head(DllList::Active, reused).unwrap();
        assert_eq!(walk(&market, DllList::Active), [0]);
        assert!(walk(&market, DllList::Free).is_empty());
    }

    #[test]
    fn vault_list_of_locates_active_and_tombstone() {
        let buf = setup();
        let mut market = load_market(&buf);
        market.link_head(DllList::Active, 0).unwrap();
        market.link_head(DllList::Tombstone, 1).unwrap();
        assert_eq!(market.vault_list_of(0), Some(DllList::Active));
        assert_eq!(market.vault_list_of(1), Some(DllList::Tombstone));
        // A detached sector is on neither list.
        assert_eq!(market.vault_list_of(2), None);
    }

    #[test]
    fn reclaim_active_sector_moves_to_free_and_decrements_count() {
        let buf = setup();
        let mut market = load_market(&buf);
        market.link_head(DllList::Active, 0).unwrap();
        market.link_head(DllList::Active, 1).unwrap();
        market.active_count = 2u32.into();
        // Stamp a non-default leader so we can assert reclaim zeroes it.
        market.as_mut_slice()[0].leader = [0x11; 32].into();

        market.reclaim_sector(0).unwrap();

        // Sector 0 left the active list and joined the free list.
        assert_eq!(walk(&market, DllList::Active), [1]);
        assert_eq!(walk(&market, DllList::Free), [0]);
        // Active count dropped by one.
        assert_eq!(market.active_count.get(), 1);
        // Emptiness marker restored.
        assert_eq!(market.as_slice()[0].leader, Address::default());
    }

    #[test]
    fn reclaim_tombstone_sector_leaves_active_count() {
        let buf = setup();
        let mut market = load_market(&buf);
        // One active vault, one tombstoned vault awaiting drain.
        market.link_head(DllList::Active, 0).unwrap();
        market.link_head(DllList::Tombstone, 1).unwrap();
        market.active_count = 1u32.into();

        market.reclaim_sector(1).unwrap();

        // Active list + count untouched; tombstone emptied; free gained 1.
        assert_eq!(walk(&market, DllList::Active), [0]);
        assert_eq!(market.active_count.get(), 1);
        assert!(walk(&market, DllList::Tombstone).is_empty());
        assert_eq!(walk(&market, DllList::Free), [1]);
    }

    #[test]
    fn reclaim_rejects_sector_on_no_list() {
        let buf = setup();
        let mut market = load_market(&buf);
        // Sector 0 is detached (on neither active nor tombstone) — a
        // double-reclaim attempt. Must error rather than corrupt free.
        let err = market
            .reclaim_sector(0)
            .expect_err("reclaim of an unthreaded sector must be rejected");
        let expected: ProgramError = DropsetError::CorruptVaultList.into();
        assert_eq!(err, expected);
        assert!(walk(&market, DllList::Free).is_empty());
    }
}
