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

/// Flush flag OR'd onto [`ReferencePrice::stamp`] by `SetReferencePrice`
/// and `SetLiquidityProfile`. The next taker materializes
/// [`Vault::remaining`] from the [`LiquidityProfile`] and clears the
/// flag — see the spec's **LiquidityProfile → Flush**.
pub const FLUSH_BIT: u64 = 1u64 << 63;

/// Default sanity cap (in slots) on how stale a leader-supplied
/// `quote_slot` may be. ~20s on Solana mainnet. Per the spec's
/// **SetReferencePrice**: backdating only shortens the effective expiry
/// window (self-grief, not exploit), but worth bounding.
pub const MAX_BACKDATE: u64 = 50;

/// Q32.32 fixed-point representation of `1.0` — the seed value for
/// [`Vault::hwm`] at first-deposit time. The HWM is value-per-share
/// (`L / total_shares`); the first depositor's basket implies
/// `L = total_shares` (since `total_shares := isqrt(b·q) == L`), so the
/// initial VPS is exactly 1.0.
pub const Q32_32_ONE: u64 = 1u64 << 32;

/// Parts-per-million denominator (`1_000_000 = 100%`).
pub const PPM: u64 = 1_000_000;

/// Basis-points denominator (`10_000 = 100%`).
pub const BPS: u64 = 10_000;

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
    /// True when an admin has frozen this vault. Alignment-1
    /// `PodBool` so the field stays at the same on-chain offset as
    /// the previous `u8` representation, but readers / writers go
    /// through `.get()` / `.into()` for strongly-typed semantics
    /// rather than `== 1` / `!= 0` comparisons.
    pub frozen: PodBool,
    /// True when the leader opted into outside deposits.
    pub allow_outside_depositors: PodBool,
    /// True when an admin approved outside deposits.
    pub outside_deposits_approved: PodBool,
    /// Explicit reserved bytes so [`Vault`] stays Pod-friendly (no
    /// implicit padding) and leaves a small slot for future flag
    /// additions without changing the on-chain size.
    pub _reserved: [u8; 5],
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
    /// Number of live `VaultDepositor` PDAs across every vault on this
    /// market (active and tombstoned). Incremented when an outside
    /// `Deposit` opens a fresh `VaultDepositor`, decremented when
    /// `Withdraw` closes one on `shares == 0` and when
    /// `force_withdraw_depositor` closes one. **Not** incremented on
    /// top-off (existing `VaultDepositor`). `close_market` requires
    /// this to be zero — the only on-chain witness that no orphan
    /// depositor PDAs remain, since the program cannot iterate all
    /// PDAs to verify by enumeration. See the architecture spec,
    /// **Account lifecycle and rent reclamation**.
    pub outstanding_vault_depositors: PodU32,
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
const _: () = assert!(core::mem::size_of::<MarketHeader>() == 237);
const _: () = assert!(core::mem::size_of::<LiquidityProfile>() == 2 * N_LEVELS * 10);
const _: () = assert!(core::mem::size_of::<Remaining>() == 2 * N_LEVELS * 16);

// Field-offset guards: total-size asserts alone don't catch a reorder
// that happens to preserve the byte count (e.g. swapping `_reserved`
// with another byte array). Pin the load-bearing offsets so the build
// breaks on any field reorder that would shift the on-chain layout —
// `next`/`prev` are dispatched directly by the DLL ops, `leader`
// doubles as the emptiness marker, and `_reserved` is the only field
// whose contents are intentionally zeroed.
const _: () = assert!(core::mem::offset_of!(Vault, next) == 0);
const _: () = assert!(core::mem::offset_of!(Vault, prev) == 4);
const _: () = assert!(core::mem::offset_of!(Vault, leader) == 8);
const _: () = assert!(core::mem::offset_of!(Vault, _reserved) == 139);
const _: () = assert!(core::mem::offset_of!(MarketHeader, head) == 8);
const _: () = assert!(core::mem::offset_of!(MarketHeader, tombstone_head) == 12);
const _: () = assert!(core::mem::offset_of!(MarketHeader, free_head) == 16);

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
/// - `vault.perf_fee_rate == 0` (no fee to accrue);
/// - `VPS <= HWM` (no excess to fee).
///
/// `L`, `HWM`, and the intermediate share math run in `u128` to keep
/// the perf-fee formula precise without overflow on realistic atom
/// scales. The final `m` is clamped back into `u64`.
pub fn realize_in_place(vault: &mut Vault) -> RealizeOutcome {
    let s = vault.total_shares.get();
    let hwm = vault.hwm.get();
    if s == 0 || vault.frozen.get() {
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: hwm,
        };
    }
    let f_ppm = vault.perf_fee_rate.get() as u128;
    let b = vault.base_atoms.get() as u128;
    let q = vault.quote_atoms.get() as u128;
    let l = isqrt_u128(b.saturating_mul(q));
    if l == 0 {
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: hwm,
        };
    }
    // `vps` in Q32.32, same encoding as `hwm`.
    let vps = (l << 32) / (s as u128);
    if vps <= hwm as u128 {
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: hwm,
        };
    }
    if f_ppm == 0 {
        // No perf fee — HWM still trails VPS upwards so a later fee
        // change can't claw back past historical highs.
        let hwm_after = vps as u64;
        vault.hwm = hwm_after.into();
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after,
        };
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
        vault.hwm = (vps as u64).into();
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: vps as u64,
        };
    }
    let num = f_ppm * s_u * (l - hwm_s);
    let one_minus_f = PPM as u128 - f_ppm;
    let denom = one_minus_f * l + f_ppm * hwm_s;
    if denom == 0 {
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: hwm,
        };
    }
    let m = (num / denom).min(u64::MAX as u128) as u64;
    if m == 0 {
        vault.hwm = (vps as u64).into();
        return RealizeOutcome {
            shares_minted: 0,
            hwm_after: vps as u64,
        };
    }
    let s_after = s.saturating_add(m);
    let leader_after = vault.leader_shares.get().saturating_add(m);
    let hwm_after = ((l << 32) / s_after as u128) as u64;
    vault.total_shares = s_after.into();
    vault.leader_shares = leader_after.into();
    vault.hwm = hwm_after.into();
    RealizeOutcome {
        shares_minted: m,
        hwm_after,
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
/// fixed number of sectors up front and exercises the
/// pointer-arithmetic surface (`link_head`, `unlink`, free-list reuse
/// of `allocate_sector`) directly against the on-chain code path.
///
/// `allocate_sector`'s tail-growth branch calls `resize_to_capacity` +
/// `top_up` (a system-program transfer CPI), neither of which has a
/// host-side mock in this scaffold. That branch is covered by the
/// `OpenVault` integration tests in a downstream PR — every other
/// `VaultDll` line lives here.
#[cfg(test)]
mod tests {
    use super::*;
    use anchor_lang_v2::{testing::AccountBuffer, AnchorAccount, Discriminator};

    /// Number of sectors pre-allocated in the test fixture. Bigger
    /// than any single test needs so the free-list interleaving
    /// scenarios all have headroom.
    const SECTORS: u32 = 4;

    /// Total buffer bytes: `RuntimeAccount` header (96) + 8-byte
    /// discriminator + `MarketHeader` + slab `len` field + `SECTORS`
    /// × `size_of::<Vault>()`. Rounded up to a constant that fits the
    /// largest test and stays on the stack comfortably.
    const BUF_BYTES: usize = 4096;

    /// Total data-region bytes (everything after `RuntimeAccount`):
    /// `discriminator + MarketHeader + len_field + SECTORS * Vault`.
    /// `Market::space_for(SECTORS)` already accounts for the slab
    /// header / len / alignment padding; add the 8-byte
    /// `#[account]` discriminator on top.
    const DATA_LEN: usize = 8 + Market::space_for(SECTORS);

    /// Build a fresh buffer with [`SECTORS`] zeroed sectors, empty
    /// list heads, and the correct discriminator. Returns the buffer
    /// (kept on the stack — caller holds it for the test lifetime).
    fn setup() -> AccountBuffer<BUF_BYTES> {
        let buf = AccountBuffer::<BUF_BYTES>::new();
        buf.init(
            [0xAA; 32],
            crate::ID.to_bytes(),
            DATA_LEN,
            /* is_signer */ false,
            /* is_writable */ true,
            /* executable */ false,
        );
        // Write the discriminator + slab `len = SECTORS`. The rest of
        // the data region is already zero from `AccountBuffer::new`,
        // which doubles as zeroed `Vault`s.
        let mut data = vec![0u8; DATA_LEN];
        data[..8].copy_from_slice(<MarketHeader as Discriminator>::DISCRIMINATOR);
        // len field lives right after the header at LEN_OFFSET, which
        // is `disc + size_of::<MarketHeader>()` for Anchor accounts.
        let len_off = 8 + core::mem::size_of::<MarketHeader>();
        data[len_off..len_off + 4].copy_from_slice(&SECTORS.to_le_bytes());
        buf.write_data(&data);
        buf
    }

    /// Load the buffer as a mutable `Market` and initialize the list
    /// heads + every sector's `next`/`prev` to `NULL_SECTOR`. Tests
    /// then build whatever list state they need on top.
    fn load_market(buf: &AccountBuffer<BUF_BYTES>) -> Market {
        let view = unsafe { buf.view() };
        // SAFETY: this is the only live wrapper over `view`'s data
        // for the test's duration.
        let mut market = unsafe { Market::load_mut(view).unwrap() };
        market.head = NULL_SECTOR.into();
        market.tombstone_head = NULL_SECTOR.into();
        market.free_head = NULL_SECTOR.into();
        for i in 0..SECTORS as usize {
            market.as_mut_slice()[i].next = NULL_SECTOR.into();
            market.as_mut_slice()[i].prev = NULL_SECTOR.into();
        }
        market
    }

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
    fn space_for_zero_matches_init_space() {
        // `space_for(0)` is the byte count used by `register_market`'s
        // `#[account(init, space = Market::space_for(0))]`; `INIT_SPACE`
        // is the same value surfaced through the `Space` trait that
        // anchor's derive consults during initialization. Pinning their
        // equality here guarantees the two paths stay in sync.
        assert_eq!(
            Market::space_for(0),
            <Market as anchor_lang_v2::Space>::INIT_SPACE
        );
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
        // a subsequent `OpenVault` reuses it (free → active), and
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

    // ── realize_in_place ────────────────────────────────────────────
    //
    // These exercise the perf-fee accrual formula directly on a
    // stack-allocated `Vault` — no slab, no AccountBuffer, no SVM. The
    // function is pure on its &mut Vault argument, so unit tests can
    // construct any (b, q, total_shares, leader_shares, hwm, fee)
    // state and assert the outcome.

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
    fn realize_noop_on_unseeded_vault() {
        let mut v = Vault::zeroed();
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
        assert_eq!(r.hwm_after, 0);
    }

    #[test]
    fn realize_noop_when_vps_at_or_below_hwm() {
        // Seeded at VPS = 1.0 (Q32_32_ONE), HWM also = 1.0.
        // b * q = 10_000 → L = 100, total_shares = 100, VPS = 1.0.
        let mut v = seeded_vault(100, 100, 100, 100, Q32_32_ONE, 100_000);
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
        assert_eq!(v.total_shares.get(), 100);
        assert_eq!(v.leader_shares.get(), 100);
    }

    #[test]
    fn realize_noop_when_frozen() {
        // Even with VPS above HWM, frozen vaults must not accrue.
        let mut v = seeded_vault(200, 200, 100, 100, Q32_32_ONE, 100_000);
        v.frozen = true.into();
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
    }

    #[test]
    fn realize_mints_shares_when_vps_exceeds_hwm() {
        // Seed with VPS = 1.0, then push b·q up so VPS > 1.0 and the
        // 10% perf fee should mint new shares to the leader.
        let mut v = seeded_vault(400, 400, 100, 100, Q32_32_ONE, 100_000);
        // L = isqrt(400 * 400) = 400, total_shares = 100 → VPS = 4.0
        // (Q32.32 = 4 * 2^32).
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

    #[test]
    fn realize_zero_fee_advances_hwm_only() {
        // With perf_fee_rate = 0 the leader earns no shares, but HWM
        // still trails up so a later fee bump cannot retroactively
        // accrue against historical highs.
        let mut v = seeded_vault(400, 400, 100, 100, Q32_32_ONE, 0);
        let r = realize_in_place(&mut v);
        assert_eq!(r.shares_minted, 0);
        assert!(r.hwm_after > Q32_32_ONE);
        assert_eq!(v.leader_shares.get(), 100);
        assert_eq!(v.total_shares.get(), 100);
    }
}
