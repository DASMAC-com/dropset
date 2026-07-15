//! Byte-exact on-chain layout for a market account: the [`MarketHeader`]
//! and the [`Vault`] sectors in its slab tail, the smaller records they
//! embed ([`ReferencePrice`], [`Level`], [`LiquidityProfile`], [`Position`],
//! [`Remaining`]), and the size / offset const-asserts that pin every one
//! of them. The asserts are kept here, beside the structs they guard, so
//! the IDL-canonical layout lives in one auditable place: any accidental
//! field reorder or `Pod*`-width change breaks the build at this file.

use anchor_lang_v2::{
    address_eq,
    bytemuck::{Pod, Zeroable},
    prelude::*,
};

use crate::{FeeConfig, Price};

use super::N_LEVELS;

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
    /// Performance fee rate in ppm. Set at `CreateVault`; immutable.
    pub perf_fee_rate: PodU32,
    /// Floor on `leader_shares / total_shares` in ppm. Stamped at
    /// `CreateVault` from `MarketHeader.default_min_leader_share`, then
    /// admin-retunable per vault via `SetMinLeaderShare`.
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
    /// True when the leader has `CloseVault`'d this vault, moving it
    /// from the active DLL to the tombstone DLL. Mirrors how `frozen`
    /// works: the flag makes "this vault is dead" a cheap local read
    /// for handlers (`realize_in_place`, both deposit paths) instead
    /// of an O(n) `vault_list_of` walk — and is the signal
    /// `withdraw_leader`'s `min_leader_share` floor will read once that
    /// floor is taught to honor it. Set in `close_vault` alongside the
    /// list move;
    /// cleared implicitly when the sector is reclaimed and reused
    /// (`allocate_sector` zeroes the whole struct). `PodBool` so the
    /// field is alignment-1 and slots into the former `_reserved`
    /// space without shifting any other offset.
    pub tombstoned: PodBool,
    /// Explicit reserved bytes so [`Vault`] stays Pod-friendly (no
    /// implicit padding) and leaves a small slot for future flag
    /// additions without changing the on-chain size.
    pub _reserved: [u8; 4],
    /// Bids / asks ladder as offsets from the reference price.
    pub profile: LiquidityProfile,
    /// Materialized per-level state (computed at flush time).
    pub remaining: Remaining,
}

impl LiquidityProfile {
    /// Per-side `Σ size_bps`, returned as `(bid_sum, ask_sum)`. A `u32`
    /// accumulator: at `N_LEVELS = 8` the upper bound is
    /// `8 × u16::MAX = 524_280`, far inside `u32` range, so the
    /// `saturating_add` never actually saturates on a real profile.
    ///
    /// Single source for the two `Σ size_bps ≤ BPS` gates that would
    /// otherwise re-derive this loop independently: the write-time reject
    /// in `set_liquidity_profile` (which rejects `Σ > BPS` before any
    /// `profile` bytes are stored) and the match-time flush gate in
    /// [`Vault::materialize_remaining`] (which zeroes an oversized side
    /// out of matching rather than aborting the taker's swap). Each caller
    /// applies its own `≤ BPS` comparison and failure behavior.
    #[inline(always)]
    pub fn side_size_sums(&self) -> (u32, u32) {
        let mut bid_sum: u32 = 0;
        let mut ask_sum: u32 = 0;
        for i in 0..N_LEVELS {
            bid_sum = bid_sum.saturating_add(self.bids[i].size_bps.get() as u32);
            ask_sum = ask_sum.saturating_add(self.asks[i].size_bps.get() as u32);
        }
        (bid_sum, ask_sum)
    }
}

impl Vault {
    /// True when this sector currently holds a live vault rather than a
    /// free-list slot. `leader == Address::default()` is the spec's
    /// emptiness marker (see [`Vault::leader`]); every handler that
    /// rejects an empty sector with `VaultEmpty` reads this predicate
    /// rather than re-deriving the `Address::default()` comparison.
    #[inline(always)]
    pub fn is_occupied(&self) -> bool {
        !address_eq(&self.leader, &Address::default())
    }

    /// True when the stamped reference price is usable for matching —
    /// constructed, finite, and non-zero. Single source of truth for
    /// the book-construction validity gate (spec § Order matching →
    /// Book construction), shared by the matching loop and any
    /// cold-path reader that needs the same notion of a live price.
    #[inline(always)]
    pub fn has_valid_reference_price(&self) -> bool {
        let p = self.reference_price.price;
        p.is_valid() && !p.is_zero() && !p.is_infinity()
    }
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
    /// Head of the free DLL: sectors available for reuse on `CreateVault`.
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
    /// Per-market create-vault fee: mint and amount. Seeded from
    /// `Registry.default_fee_config` at market creation, then
    /// admin-retunable via `SetMarketFeeConfig`.
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
}

// Size regression guards: `#[derive(Pod)]` already rejects implicit
// padding, but it can't catch a field reorder that lands at the same
// total size by accident, nor a silent bump to a `Pod*` wrapper width.
// These const asserts pin the on-chain layout — any change must be a
// deliberate update here, paired with the matching account-data
// migration story.
const _: () = assert!(core::mem::size_of::<Vault>() == 560);
const _: () = assert!(core::mem::size_of::<MarketHeader>() == 235);
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
const _: () = assert!(core::mem::offset_of!(Vault, tombstoned) == 139);
const _: () = assert!(core::mem::offset_of!(Vault, _reserved) == 140);
const _: () = assert!(core::mem::offset_of!(MarketHeader, head) == 8);
const _: () = assert!(core::mem::offset_of!(MarketHeader, tombstone_head) == 12);
const _: () = assert!(core::mem::offset_of!(MarketHeader, free_head) == 16);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_occupied_tracks_leader_marker() {
        let mut v = Vault::zeroed();
        // Free-list slot: default leader.
        assert!(!v.is_occupied());
        v.leader = [0x11; 32].into();
        assert!(v.is_occupied());
    }

    #[test]
    fn has_valid_reference_price_rejects_sentinels_and_garbage() {
        let mut v = Vault::zeroed();
        // Accept: a constructed, finite, non-zero price.
        v.reference_price.price = Price::from_value(1.0).unwrap();
        assert!(v.has_valid_reference_price());
        // Reject: the ZERO sentinel (valid encoding, but not a price).
        v.reference_price.price = Price::from_bits(0);
        assert!(!v.has_valid_reference_price());
        // Reject: the INFINITY sentinel.
        v.reference_price.price = Price::from_bits(u32::MAX);
        assert!(!v.has_valid_reference_price());
        // Reject: a non-sentinel with an out-of-range significand —
        // `is_valid()` is false, so it never anchors a ladder.
        v.reference_price.price = Price::from_bits(1);
        assert!(!v.has_valid_reference_price());
    }
}
