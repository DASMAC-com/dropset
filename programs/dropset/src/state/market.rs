//! Per-market state: [`MarketHeader`] + a [`Slab`]-tail of [`Vault`]
//! sectors threaded into three doubly-linked lists (active / tombstoned /
//! free). See the architecture spec's **MarketHeader**, **Storage layout**,
//! and **Vault** sections.

use anchor_lang_v2::{
    accounts::Slab,
    bytemuck::{Pod, Zeroable},
    prelude::*,
};

use crate::{errors::DropsetError, FeeConfig, Price};

/// Number of bid / ask levels in a [`LiquidityProfile`]. Chosen small for
/// the initial bring-up; widen once the matching engine lands and CU
/// budgets are measured.
pub const N_LEVELS: usize = 8;

/// "Null" sentinel for sector-index pointers ([`MarketHeader::head`],
/// [`Vault::next`], etc.). Sector indices are at most
/// `registry.max_vaults_per_market` (`u8`), so [`u32::MAX`] is unreachable
/// as a real index and works as a null marker.
pub const NULL_SECTOR: u32 = u32::MAX;

/// Reference-price record stamped onto every vault. See the spec's
/// **Vault → ReferencePrice**.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct ReferencePrice {
    /// `market.nonce` at the last `SetReferencePrice` / `SetLiquidityProfile`,
    /// OR'd with `FLUSH_BIT` when a flush is armed. Alignment-1.
    pub stamp: PodU64,
    /// Reference price the leader's ladder is anchored to.
    pub price: Price,
    /// Slot the quote was "as of" (leader-supplied, validated at write
    /// time).
    pub quote_slot: PodU32,
}

/// One level in a [`LiquidityProfile`]. All fields are alignment-1 so the
/// containing array is byte-packed.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Level {
    /// Spread from `reference_price.price` in ppm — direction is implicit
    /// from which side this level sits on (bids subtract, asks add).
    pub price_offset: PodU32,
    /// Per-flush allowance as bps of the matching inventory leg
    /// (`base_atoms` for asks, `quote_atoms` for bids). Σ per side ≤ 10000.
    pub size_bps: PodU16,
    /// Per-level expiry in slots after `reference_price.quote_slot`.
    pub expiry_offset: PodU32,
}

/// The leader's bid / ask ladder, expressed as offsets from a single
/// reference price.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct LiquidityProfile {
    /// Bid levels, top of book first.
    pub bids: [Level; N_LEVELS],
    /// Ask levels, top of book first.
    pub asks: [Level; N_LEVELS],
}

/// Materialized per-level state: absolute price, atom-sized allowance, and
/// absolute expiry. Populated lazily by the first taker after a flush is
/// armed.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Position {
    /// Absolute price for this level.
    pub price: Price,
    /// Live allowance in atoms (base for asks, quote for bids).
    pub size: PodU64,
    /// Absolute slot this level expires at.
    pub expires_at: PodU32,
}

/// Per-vault remaining sizes, one entry per [`Level`].
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Remaining {
    pub bids: [Position; N_LEVELS],
    pub asks: [Position; N_LEVELS],
}

/// A vault sector — a leader's pooled inventory, ladder, and reference
/// price, plus DLL pointers threading it into one of three lists tracked
/// by the [`MarketHeader`]. See the spec's **Vault** and **Storage
/// layout**.
///
/// The pointer fields ([`Vault::next`] / [`Vault::prev`]) are sector
/// indices: a position within the slab tail, **not** a byte offset.
/// [`NULL_SECTOR`] marks the end of a list.
///
/// [`Vault::leader`] doubles as the emptiness marker per the spec — a
/// sector with `leader == Address::default()` is on the free list.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Vault {
    /// Next sector in the current DLL (active / tombstone / free), or
    /// [`NULL_SECTOR`] at the tail.
    pub next: PodU32,
    /// Previous sector in the current DLL, or [`NULL_SECTOR`] at the
    /// head. Free-list sectors leave this unused.
    pub prev: PodU32,
    /// Leader pubkey. `Address::default()` means "on the free list".
    pub leader: Address,
    /// Authority for quote-mutating ix; always populated. See the spec's
    /// **Vault** for rotation semantics.
    pub quote_authority: Address,
    /// Packed `(stamp, price, quote_slot)` — hot path on
    /// `SetReferencePrice`.
    pub reference_price: ReferencePrice,
    /// Pooled base inventory across the leader and outside depositors.
    pub base_atoms: PodU64,
    /// Pooled quote inventory across the leader and outside depositors.
    pub quote_atoms: PodU64,
    /// `leader_shares + Σ VaultDepositor.shares`.
    pub total_shares: PodU64,
    /// Leader's stake (non-SPL; see **Shares**).
    pub leader_shares: PodU64,
    /// High-water mark of `L / total_shares` as Q32.32.
    pub hwm: PodU64,
    /// Performance fee rate in ppm. Set at `OpenVault`; immutable.
    pub perf_fee_rate: PodU32,
    /// Floor on `leader_shares / total_shares` in ppm. Stamped at
    /// `OpenVault` from `MarketHeader.default_min_leader_share`.
    pub min_leader_share: PodU32,
    /// Set to 1 when an admin has frozen this vault.
    pub frozen: u8,
    /// Set to 1 when the leader opted into outside deposits.
    pub allow_outside_depositors: u8,
    /// Set to 1 when an admin approved outside deposits.
    pub outside_deposits_approved: u8,
    /// Explicit pad so [`Vault`] stays Pod-friendly (no implicit
    /// padding). Zero-initialized.
    pub _pad: [u8; 5],
    /// Bids / asks ladder as offsets from the reference price.
    pub profile: LiquidityProfile,
    /// Materialized per-level state (computed at flush time).
    pub remaining: Remaining,
}

/// Header of a market account. Followed by a slab tail of [`Vault`]
/// sectors. Per-market knobs are seeded from the registry at creation
/// and tunable downstream by admins.
///
/// All fields are alignment-1 — `Address`, `Pod*` wrappers, `[FeeConfig]`,
/// `u8` — so the header is padding-free and casts directly from the
/// account bytes.
#[account]
pub struct MarketHeader {
    /// Per-fill / per-quote monotonic counter.
    pub nonce: PodU64,
    /// Head of the active DLL: sector index or [`NULL_SECTOR`]. Walked
    /// by the matching engine on every taker.
    pub head: PodU32,
    /// Head of the tombstone DLL: sectors that have been `CloseVault`'d
    /// but still hold outstanding shares. Not visited by matching.
    pub tombstone_head: PodU32,
    /// Head of the free DLL: sectors available for reuse on `OpenVault`.
    /// Singly linked via `next`; `prev` is ignored.
    pub free_head: PodU32,
    /// Active-DLL length. Bounded by `registry.max_vaults_per_market`.
    pub active_count: PodU32,
    /// Per-market open-vault fee: mint and amount. Seeded from
    /// `Registry.default_fee_config` at market creation.
    pub fee_config: FeeConfig,
    /// Taker fee rate, capped at ~6.55% (`Ppm16` max).
    pub taker_fee: PodU16,
    /// Default min-leader-share for vaults opened on this market.
    /// Stamped from `Registry.default_min_leader_share` at creation.
    pub default_min_leader_share: PodU32,
    /// Base leg mint.
    pub base_mint: Address,
    /// Quote leg mint.
    pub quote_mint: Address,
    /// SPL / Token-2022 token account holding pooled base inventory.
    /// ATA derived from `(market_pda, base_mint, base_token_program)`.
    pub base_treasury: Address,
    /// Same as `base_treasury`, for the quote leg.
    pub quote_treasury: Address,
    /// Market PDA bump.
    pub bump: u8,
    /// Bump for the base treasury ATA derivation. Stored so transfers
    /// out can sign with the market PDA without re-deriving.
    pub base_treasury_bump: u8,
    /// Bump for the quote treasury ATA derivation.
    pub quote_treasury_bump: u8,
}

/// Market account: [`MarketHeader`] followed by a slab tail of [`Vault`]
/// sectors. Sectors are managed via the [`VaultDll`] operations rather
/// than the raw slab `push` / `swap_remove` — those would break the DLL
/// invariants the matching engine relies on.
pub type Market = Slab<MarketHeader, Vault>;

// Size regression guards: `#[derive(Pod)]` already rejects implicit
// padding, but it can't catch a field reorder that lands at the same
// total size by accident, nor a silent bump to a `Pod*` wrapper width.
// These const asserts pin the on-chain layout — any change must be a
// deliberate update here, paired with the matching account-data
// migration story.
const _: () = assert!(core::mem::size_of::<Vault>() == 560);
const _: () = assert!(core::mem::size_of::<MarketHeader>() == 233);
const _: () = assert!(core::mem::size_of::<LiquidityProfile>() == 2 * N_LEVELS * 10);
const _: () = assert!(core::mem::size_of::<Remaining>() == 2 * N_LEVELS * 16);

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

    /// Walk `list` starting at its head; returns the visit order as a
    /// `Vec<u32>`. Helper for unit tests; production paths iterate
    /// inline.
    #[cfg(test)]
    fn walk(&self, list: DllList) -> alloc::vec::Vec<u32>;
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

    #[cfg(test)]
    fn walk(&self, list: DllList) -> alloc::vec::Vec<u32> {
        let mut out = alloc::vec::Vec::new();
        let mut cur = list.head(self);
        while cur != NULL_SECTOR {
            out.push(cur);
            cur = self.as_slice()[cur as usize].next.get();
        }
        out
    }
}

#[cfg(test)]
extern crate alloc;

/// Cross-list pointer-table unit tests — exercise the DLL operations in
/// isolation from the rest of the program. They drive a hand-built
/// `MarketHeader` + `Vec<Vault>` through the same `VaultDll` algorithms
/// the on-chain `Market` slab uses, so the on-chain account layout
/// doesn't have to be stood up to assert the invariants.
#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    /// In-memory stand-in for the slab tail: a `MarketHeader` plus a
    /// dynamic `Vec<Vault>` we can grow as new sectors are allocated.
    /// `VaultDll` is normally implemented on `Slab<MarketHeader, Vault>`,
    /// but the algorithm only needs *some* container that exposes a
    /// `MarketHeader` and an indexable `[Vault]` slice — so we mirror
    /// that surface here to test the algorithm directly.
    struct Tape {
        header: MarketHeader,
        vaults: Vec<Vault>,
    }

    impl Tape {
        fn new() -> Self {
            let mut header = MarketHeader::zeroed();
            header.head = NULL_SECTOR.into();
            header.tombstone_head = NULL_SECTOR.into();
            header.free_head = NULL_SECTOR.into();
            Self {
                header,
                vaults: Vec::new(),
            }
        }

        fn allocate(&mut self) -> u32 {
            let free_head = self.header.free_head.get();
            if free_head != NULL_SECTOR {
                let next = self.vaults[free_head as usize].next.get();
                self.header.free_head = next.into();
                self.vaults[free_head as usize] = Vault::zeroed();
                self.vaults[free_head as usize].next = NULL_SECTOR.into();
                self.vaults[free_head as usize].prev = NULL_SECTOR.into();
                return free_head;
            }
            let idx = self.vaults.len() as u32;
            let mut v = Vault::zeroed();
            v.next = NULL_SECTOR.into();
            v.prev = NULL_SECTOR.into();
            self.vaults.push(v);
            idx
        }

        fn link_head(&mut self, list: DllList, sector: u32) {
            let prev_head = list.head(&self.header);
            self.vaults[sector as usize].next = prev_head.into();
            self.vaults[sector as usize].prev = NULL_SECTOR.into();
            if prev_head != NULL_SECTOR {
                self.vaults[prev_head as usize].prev = sector.into();
            }
            list.set_head(&mut self.header, sector);
        }

        fn unlink(&mut self, list: DllList, sector: u32) {
            let next = self.vaults[sector as usize].next.get();
            let prev = self.vaults[sector as usize].prev.get();
            if prev != NULL_SECTOR {
                self.vaults[prev as usize].next = next.into();
            } else {
                list.set_head(&mut self.header, next);
            }
            if next != NULL_SECTOR {
                self.vaults[next as usize].prev = prev.into();
            }
            self.vaults[sector as usize].next = NULL_SECTOR.into();
            self.vaults[sector as usize].prev = NULL_SECTOR.into();
        }

        fn walk(&self, list: DllList) -> Vec<u32> {
            let mut out = Vec::new();
            let mut cur = list.head(&self.header);
            while cur != NULL_SECTOR {
                out.push(cur);
                cur = self.vaults[cur as usize].next.get();
            }
            out
        }
    }

    #[test]
    fn empty_lists_walk_to_nothing() {
        let tape = Tape::new();
        assert!(tape.walk(DllList::Active).is_empty());
        assert!(tape.walk(DllList::Tombstone).is_empty());
        assert!(tape.walk(DllList::Free).is_empty());
    }

    #[test]
    fn allocate_returns_sequential_indices() {
        let mut tape = Tape::new();
        assert_eq!(tape.allocate(), 0);
        assert_eq!(tape.allocate(), 1);
        assert_eq!(tape.allocate(), 2);
    }

    #[test]
    fn link_head_prepends_most_recent() {
        let mut tape = Tape::new();
        for _ in 0..3 {
            let s = tape.allocate();
            tape.link_head(DllList::Active, s);
        }
        // Spec: "New vaults are prepended at `head`" — most recent first.
        assert_eq!(tape.walk(DllList::Active), [2, 1, 0]);
    }

    #[test]
    fn unlink_middle_patches_neighbors() {
        let mut tape = Tape::new();
        for _ in 0..3 {
            let s = tape.allocate();
            tape.link_head(DllList::Active, s);
        }
        // Order: 2 <-> 1 <-> 0. Unlink the middle.
        tape.unlink(DllList::Active, 1);
        assert_eq!(tape.walk(DllList::Active), [2, 0]);
        assert_eq!(tape.vaults[2].next.get(), 0);
        assert_eq!(tape.vaults[0].prev.get(), 2);
        // The detached sector is left with NULL pointers so it's safe to
        // re-thread onto another list.
        assert_eq!(tape.vaults[1].next.get(), NULL_SECTOR);
        assert_eq!(tape.vaults[1].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_head_promotes_next() {
        let mut tape = Tape::new();
        for _ in 0..3 {
            let s = tape.allocate();
            tape.link_head(DllList::Active, s);
        }
        // Order: 2 <-> 1 <-> 0. Unlink the head (sector 2).
        tape.unlink(DllList::Active, 2);
        assert_eq!(tape.walk(DllList::Active), [1, 0]);
        assert_eq!(tape.header.head.get(), 1);
        assert_eq!(tape.vaults[1].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_tail_leaves_predecessor_at_end() {
        let mut tape = Tape::new();
        for _ in 0..3 {
            let s = tape.allocate();
            tape.link_head(DllList::Active, s);
        }
        // Order: 2 <-> 1 <-> 0. Unlink the tail (sector 0).
        tape.unlink(DllList::Active, 0);
        assert_eq!(tape.walk(DllList::Active), [2, 1]);
        assert_eq!(tape.vaults[1].next.get(), NULL_SECTOR);
    }

    #[test]
    fn unlink_only_element_empties_list() {
        let mut tape = Tape::new();
        let s = tape.allocate();
        tape.link_head(DllList::Active, s);
        tape.unlink(DllList::Active, s);
        assert!(tape.walk(DllList::Active).is_empty());
        assert_eq!(tape.header.head.get(), NULL_SECTOR);
    }

    #[test]
    fn active_to_tombstone_moves_independently() {
        let mut tape = Tape::new();
        let a = tape.allocate();
        let b = tape.allocate();
        tape.link_head(DllList::Active, a);
        tape.link_head(DllList::Active, b);
        // CloseVault flow: unlink from active, prepend to tombstone.
        tape.unlink(DllList::Active, a);
        tape.link_head(DllList::Tombstone, a);
        assert_eq!(tape.walk(DllList::Active), [b]);
        assert_eq!(tape.walk(DllList::Tombstone), [a]);
        // The two lists must not share members or pointers.
        assert_eq!(tape.vaults[a as usize].next.get(), NULL_SECTOR);
        assert_eq!(tape.vaults[b as usize].prev.get(), NULL_SECTOR);
    }

    #[test]
    fn free_list_recycles_sector_on_next_allocate() {
        let mut tape = Tape::new();
        let a = tape.allocate();
        let b = tape.allocate();
        tape.link_head(DllList::Active, a);
        tape.link_head(DllList::Active, b);
        // Reclaim flow: unlink from whichever DLL, push onto free list.
        tape.unlink(DllList::Active, a);
        tape.link_head(DllList::Free, a);
        // Next allocate must reuse the freed sector rather than grow.
        let reused = tape.allocate();
        assert_eq!(reused, a);
        // Free list is drained.
        assert_eq!(tape.header.free_head.get(), NULL_SECTOR);
        // Slab didn't grow beyond the original two sectors.
        assert_eq!(tape.vaults.len(), 2);
    }

    #[test]
    fn free_list_lifo_order() {
        let mut tape = Tape::new();
        let a = tape.allocate();
        let b = tape.allocate();
        let c = tape.allocate();
        // Push a, then b, then c onto the free list. LIFO: c, b, a.
        tape.link_head(DllList::Free, a);
        tape.link_head(DllList::Free, b);
        tape.link_head(DllList::Free, c);
        assert_eq!(tape.allocate(), c);
        assert_eq!(tape.allocate(), b);
        assert_eq!(tape.allocate(), a);
        // All consumed — next allocate must grow.
        assert_eq!(tape.allocate(), 3);
    }

    #[test]
    fn reallocated_sector_starts_clean() {
        let mut tape = Tape::new();
        let a = tape.allocate();
        // Scribble on it then reclaim.
        tape.vaults[a as usize].base_atoms = 12345u64.into();
        tape.vaults[a as usize].leader_shares = 999u64.into();
        tape.link_head(DllList::Free, a);
        let reused = tape.allocate();
        assert_eq!(reused, a);
        assert_eq!(tape.vaults[a as usize].base_atoms.get(), 0);
        assert_eq!(tape.vaults[a as usize].leader_shares.get(), 0);
    }
}
