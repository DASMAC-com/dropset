//! Byte-exact on-chain layout for a market account: the [`MarketHeader`]
//! and the [`Vault`] sectors in its slab tail, the smaller records they
//! embed ([`ReferencePrice`], [`Level`], [`LiquidityProfile`], [`Position`],
//! [`Remaining`]), and the size / offset const-asserts that pin every one
//! of them. The asserts are kept here, beside the structs they guard, so
//! the IDL-canonical layout lives in one auditable place: any accidental
//! field reorder or `Pod*`-width change breaks the build at this file.
//!
//! # Expiry is dual-domain
//!
//! A level is live only while it is inside *both* its slot bound and its
//! wall bound ([`Level`] carries one offset per domain, [`Position`] one
//! materialized deadline per domain). The two answer different failure
//! modes and neither subsumes the other.
//!
//! - The **wall** bound is what survives a halt. Slots stop ticking while
//!   the cluster is down, so a slot-only ladder returns at restart with
//!   its full budget intact, anchored to a pre-halt price — hours of
//!   price movement delivered into one block, against spreads that assume
//!   price continuity.
//! - The **slot** bound is what gives a tight level a *fast* deadline.
//!   `Clock.unix_timestamp` is a stake-weighted median of vote timestamps
//!   and accurate only to a few seconds, which floors any wall TIF at
//!   ~15 s. A top-of-book level wants a sub-second dead-man tail behind
//!   the quoter's latest stamp; two slots expresses that, 15 seconds
//!   cannot.
//!
//! Taking the **min of two leader-supplied bounds** is never worse than
//! either alone, and it is robust across clock regimes: under today's
//! median-and-clamp clock the wall bound kills across a halt, and under a
//! leader-stamped clock (SIMD-0363 direction, unratified) where cluster
//! time recovers an outage at ~2x pace rather than jumping, the slot
//! conjunct is the fast protection instead.
//!
//! Both datums are stamped at **quote-write time** and never derived at
//! materialize time. Materialization runs lazily inside the first taker's
//! swap after `FLUSH_BIT` arms, so a materialize-time stamp would be
//! attacker-scheduled — in the halt scenario the first post-restart taker
//! *is* the pick-off flow, and its own transaction would refresh the very
//! quote it is picking off. This constraint applies to both domains; see
//! the spec's **SetReferencePrice**.

use anchor_lang_v2::{
    address_eq,
    bytemuck::{Pod, Zeroable},
    prelude::*,
};

use crate::{FeeConfig, Price, SlotSpan, SlotTime, WallSpan, WallTime};

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
    /// Slot the quote was "as of" (leader-supplied). The **slot datum**
    /// every level's [`Level::expiry_offset_slots`] is measured from.
    pub quote_slot: PodU32,
    /// Wall-clock time the quote was "as of", in unix seconds
    /// (leader-supplied). The **wall datum** every level's
    /// [`Level::expiry_offset_secs`] is measured from.
    pub quote_unix: PodU32,
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
    /// Per-level expiry in **seconds** after
    /// `reference_price.quote_unix`. Expiry stratification is the
    /// passive kill switch: tight levels are given seconds, deep levels
    /// minutes, so a dead leader's book decays level by level in wall
    /// terms instead of sharing one quote-wide deadline.
    ///
    /// **Zero is dead.** A level with no life in *this* domain never
    /// matches, whatever its datum — materialization encodes that as the
    /// zero sentinel in [`Position::expires_at_unix`] (and the slot
    /// offset likewise in [`Position::expires_at_slot`]).
    pub expiry_offset_secs: PodU32,
    /// Per-level expiry in **slots** after `reference_price.quote_slot`
    /// — the second, independent bound (see the note under
    /// [`ReferencePrice`]). Also zero-is-dead.
    ///
    /// "No slot bound" is expressed as the **maximum** offset, not a
    /// sentinel, so the gate stays a single unconditional compare. The
    /// `u32` ceiling is ~4.3e9 slots — decades even at the fastest
    /// proposed slot times, and so comfortably past the longest wall TIF
    /// any tier policy would set that "unbounded" means what it says.
    pub expiry_offset_slots: PodU32,
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
    /// Absolute unix second this level expires at
    /// (`reference_price.quote_unix + Level::expiry_offset_secs`,
    /// saturating).
    ///
    /// **Zero is dead**, and materialization writes zero deliberately
    /// whenever the level's own offset is zero — so "no life in this
    /// domain" survives into the stored state instead of being lost to
    /// the addition, and the match gate stays one unconditional compare.
    /// A ladder armed before any reference price is all-zero and dead by
    /// the same encoding.
    pub expires_at_unix: PodU32,
    /// Absolute slot this level expires at
    /// (`reference_price.quote_slot + Level::expiry_offset_slots`,
    /// saturating). Same zero-is-dead encoding as
    /// [`Position::expires_at_unix`]; the level matches only while **both**
    /// deadlines are in the future.
    pub expires_at_slot: PodU32,
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
/// [`super::NULL_SECTOR`] marks the end of a list.
///
/// [`Vault::leader`] doubles as the emptiness marker per the spec — a
/// sector with `leader == Address::default()` is on the free list.
#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable, IdlType)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Vault {
    /// Next sector in the current DLL (active / tombstone / free), or
    /// [`super::NULL_SECTOR`] at the tail.
    pub next: PodU32,
    /// Previous sector in the current DLL, or [`super::NULL_SECTOR`] at
    /// the head. Free-list sectors leave this unused.
    pub prev: PodU32,
    /// Leader pubkey. `Address::default()` means "on the free list".
    pub leader: Address,
    /// Authority for quote-mutating ix; always populated. See the spec's
    /// **Vault** for rotation semantics.
    pub quote_authority: Address,
    /// Packed `(stamp, price, quote_slot, quote_unix)` — the two expiry
    /// datums plus the price, written together on `SetReferencePrice`.
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

impl ReferencePrice {
    /// The slot-domain datum, typed — the point every
    /// [`Level::expiry_offset_slots`] is measured from.
    ///
    /// Prefer this over reading [`Self::quote_slot`] raw. Past it the
    /// value carries its domain in the type and cannot be handed to the
    /// wall-clock half of the gate; the raw field stays for the write
    /// path and the byte-level tests, which are wire code the types
    /// cannot help. See [`crate::clock`].
    #[inline(always)]
    pub fn slot_datum(&self) -> SlotTime {
        SlotTime::new(self.quote_slot.get())
    }
    /// The wall-domain datum, typed — the point every
    /// [`Level::expiry_offset_secs`] is measured from.
    #[inline(always)]
    pub fn wall_datum(&self) -> WallTime {
        WallTime::new(self.quote_unix.get())
    }
}

impl Level {
    /// This level's wall-domain time-in-force, typed. Zero is dead.
    #[inline(always)]
    pub fn wall_span(&self) -> WallSpan {
        WallSpan::new(self.expiry_offset_secs.get())
    }
    /// This level's slot-domain time-in-force, typed. Zero is dead;
    /// [`SlotSpan::UNBOUNDED`] leaves the level bounded only in wall
    /// time.
    #[inline(always)]
    pub fn slot_span(&self) -> SlotSpan {
        SlotSpan::new(self.expiry_offset_slots.get())
    }

    /// Both absolute deadlines for this level against `reference`'s
    /// datums — the flush-time transform, in one call.
    ///
    /// Takes the whole [`ReferencePrice`] rather than the two datums
    /// separately **on purpose**: the pair is the one thing a caller
    /// could previously transpose, and there is now no way to express
    /// the transposition. The per-domain saturating, zero-is-dead
    /// arithmetic is [`SlotTime::deadline_after`] /
    /// [`WallTime::deadline_after`] in `dropset-math-core`, which the
    /// off-chain mirror calls too — so the engine and the simulator
    /// cannot drift on the sentinel handling.
    #[inline(always)]
    pub fn deadlines(&self, reference: &ReferencePrice) -> (WallTime, SlotTime) {
        (
            reference.wall_datum().deadline_after(self.wall_span()),
            reference.slot_datum().deadline_after(self.slot_span()),
        )
    }
}

impl Position {
    /// The stored wall-domain deadline, typed.
    #[inline(always)]
    pub fn wall_deadline(&self) -> WallTime {
        WallTime::new(self.expires_at_unix.get())
    }
    /// The stored slot-domain deadline, typed.
    #[inline(always)]
    pub fn slot_deadline(&self) -> SlotTime {
        SlotTime::new(self.expires_at_slot.get())
    }
    /// Write both deadlines back, each into its own domain's field.
    ///
    /// A typed setter rather than two field assignments: assigning a
    /// slot deadline into `expires_at_unix` is exactly the mutation the
    /// PR #310 review demonstrated the suite could not see, and taking
    /// the pair as `(WallTime, SlotTime)` makes it a type error.
    #[inline(always)]
    pub fn set_deadlines(&mut self, wall: WallTime, slot: SlotTime) {
        self.expires_at_unix = wall.get().into();
        self.expires_at_slot = slot.get().into();
    }
}

impl LiquidityProfile {
    /// Per-side `Σ size_bps`, returned as `(bid_sum, ask_sum)`. A `u32`
    /// accumulator: at `N_LEVELS = 8` the upper bound is
    /// `8 × u16::MAX = 524_280`, far inside `u32` range, so the
    /// `saturating_add` never actually saturates on a real profile.
    ///
    /// Feeds the one on-chain gate on that invariant: the match-time flush
    /// in [`Vault::materialize_remaining`], which zeroes an oversized side
    /// out of matching rather than aborting the taker's swap. The write path
    /// (`set_liquidity_profile`) stores the ladder raw and does not call
    /// this — off-chain, the SDK simulator and the bot's ladder builder
    /// carry their own mirrors of the same sum, so an honest leader never
    /// arms an over-cap side in the first place.
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
    /// constructed, finite, and non-zero. The named form of the
    /// book-construction validity gate (spec § Order matching → Book
    /// construction), read by the matching loop and any cold-path
    /// reader that needs the same notion of a live price.
    ///
    /// The predicate itself is [`Price::is_matchable`], so the
    /// off-chain readers that ask the same question — the SDK's AMM
    /// adapter, the maker bot's kill stamp, the taker bot's
    /// is-anyone-quoting check — share one definition with the matcher
    /// rather than each re-deriving the three clauses.
    #[inline(always)]
    pub fn has_valid_reference_price(&self) -> bool {
        self.reference_price.price.is_matchable()
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
    /// Head of the active DLL: sector index or `NULL_SECTOR`. Walked
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
    /// Ceiling on the caller-declared platform fee, in **bps** (`Bps16`) —
    /// note the different denominator from `taker_fee` above. Seeded from
    /// `Registry.default_max_platform_fee` at creation, then admin-retunable
    /// via `SetMaxPlatformFee`.
    ///
    /// A `u16` reaches past `BPS`, so unlike `taker_fee` the type is not the
    /// bound: every write path range-checks `<= BPS` so a market can never
    /// hold a ceiling above 100% of the taker's output.
    pub max_platform_fee: PodU16,
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
    /// Protocol revenue accrued in `base_treasury`: the running sum of
    /// every taker fee charged on a base output leg (a taker `Buy`).
    /// These atoms sit physically in the treasury but belong to the
    /// protocol, not to the vaults' depositors — the treasury custody
    /// invariant is
    /// `base_treasury.amount >= Σ vault.base_atoms + accrued_base_fee_atoms`.
    /// Authoritative: nothing infers protocol revenue from a residual, so
    /// a treasury balance above the sum of the two is unattributed
    /// residual — exact-in fill change, or an unsolicited transfer — and
    /// never income (see `sweep_residual`).
    pub accrued_base_fee_atoms: PodU64,
    /// Same as `accrued_base_fee_atoms`, for the quote leg (a taker `Sell`).
    pub accrued_quote_fee_atoms: PodU64,
    /// Market PDA bump.
    pub bump: u8,
}

impl MarketHeader {
    /// Owned parts of the market PDA's signer seeds — the two leg mints
    /// and the single-byte bump, in the exact PDA-derivation order
    /// `[base_mint, quote_mint], [bump]`. Every treasury-signing CPI
    /// site (both `withdraw` legs, both `withdraw_leader` legs, all four
    /// `force_withdraw` legs, `swap`'s return leg, and `close_market`'s
    /// `CloseAccount`) reconstructs the borrowed `[&[&[u8]]; 1]` slice
    /// array from these parts on its own stack — a fully-owning helper
    /// can't return that array, since it would reference locals it
    /// drops. Keeping the seed order and count defined here once guards
    /// against a per-site drift, which would be a silent signing failure
    /// at runtime rather than a compile error.
    #[inline(always)]
    pub fn signer_seed_parts(&self) -> ([Address; 2], [u8; 1]) {
        ([self.base_mint, self.quote_mint], [self.bump])
    }
}

// Size regression guards: `#[derive(Pod)]` already rejects implicit
// padding, but it can't catch a field reorder that lands at the same
// total size by accident, nor a silent bump to a `Pod*` wrapper width.
// These const asserts pin the on-chain layout — any change must be a
// deliberate update here, paired with an account-data answer. While the
// program is pre-launch that answer is **re-create, not migrate**: a
// layout change shifts every field above it, so a market account written
// by an older build decodes as garbage rather than failing loudly, and
// the only safe response is to tear the market down and bootstrap it
// again (localnet does this on every run). A deploy that must preserve
// live accounts needs a real migration or a version gate before it
// changes anything below.
const _: () = assert!(core::mem::size_of::<Vault>() == 692);
const _: () = assert!(core::mem::size_of::<MarketHeader>() == 253);
const _: () = assert!(core::mem::size_of::<LiquidityProfile>() == 2 * N_LEVELS * 14);
const _: () = assert!(core::mem::size_of::<Remaining>() == 2 * N_LEVELS * 20);

// Field-offset guards: total-size asserts alone don't catch a reorder
// that happens to preserve the byte count (e.g. swapping `_reserved`
// with another byte array). Pin the load-bearing offsets so the build
// breaks on any field reorder that would shift the on-chain layout —
// `next`/`prev` are dispatched directly by the DLL ops, `leader`
// doubles as the emptiness marker, `_reserved` is the only field
// whose contents are intentionally zeroed, and `profile` is pinned by
// the hand-written `entrypoint.s` — its `sol_memcpy_` destination is a
// literal offset there, so a shift that only Rust knows about is a
// silent ASM/Rust divergence rather than a build break.
const _: () = assert!(core::mem::offset_of!(Vault, next) == 0);
const _: () = assert!(core::mem::offset_of!(Vault, prev) == 4);
const _: () = assert!(core::mem::offset_of!(Vault, leader) == 8);
const _: () = assert!(core::mem::offset_of!(Vault, tombstoned) == 143);
const _: () = assert!(core::mem::offset_of!(Vault, _reserved) == 144);
const _: () = assert!(core::mem::offset_of!(Vault, profile) == 148);
const _: () = assert!(core::mem::offset_of!(MarketHeader, head) == 8);
const _: () = assert!(core::mem::offset_of!(MarketHeader, tombstone_head) == 12);
const _: () = assert!(core::mem::offset_of!(MarketHeader, free_head) == 16);

// ── Clock-datum adjacency pins ───────────────────────────────────────
//
// Each domain-pair of same-width `u32`s is kept **adjacent, slot first**
// so the pair fills one 8-byte window and the hot path can move it as a
// single fused `ldxdw`/`stxdw` rather than two `ldxw`/`stxw` pairs. This
// is the general hot-path rule — *registers take a full `u64`, so
// adjacent 32-bit fields move as one 64-bit copy wherever layout allows*
// — recorded in architecture.md § **SetReferencePrice → ASM fast path**.
//
// The `ReferencePrice` pin is load-bearing in a way the size asserts
// above are not: a reorder that split that pair would leave every size
// assert green, leave Rust correct, and silently leave the assembly's
// fused copy **mis-targeted** — writing the wall datum into the slot
// field. (Mis-targeted, not misaligned: the store stays 8-byte-shaped
// and lands where it always did; it is the *fields* underneath that
// moved.) The `offset_of!` test that pins the assembly's absolute
// offsets catches a shift of the whole record; this catches a swap
// *within* it.
//
// The `Level` and `Position` pins are a weaker claim, and deliberately
// so — the assembly reads neither. `Level` crosses in the profile blob
// via one `sol_memcpy_`, and `Position` is written by Rust at flush
// time. They are pinned to keep all three domain pairs shaped alike, so
// a future hot path can fuse those too without a layout change, and so
// a reorder is a build break rather than a silent divergence between
// the two mirrors.
const _: () = assert!(
    core::mem::offset_of!(ReferencePrice, quote_unix)
        == core::mem::offset_of!(ReferencePrice, quote_slot) + 4
);
const _: () = assert!(
    core::mem::offset_of!(Level, expiry_offset_slots)
        == core::mem::offset_of!(Level, expiry_offset_secs) + 4
);
const _: () = assert!(
    core::mem::offset_of!(Position, expires_at_slot)
        == core::mem::offset_of!(Position, expires_at_unix) + 4
);

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

    #[test]
    fn signer_seed_parts_pins_order_and_count() {
        let mut m = MarketHeader::zeroed();
        m.base_mint = [0xAA; 32].into();
        m.quote_mint = [0xBB; 32].into();
        m.bump = 0xCD;
        let (mints, bump) = m.signer_seed_parts();
        // Exactly two mint seeds followed by a one-byte bump, base
        // before quote — the order a treasury CPI signs with. A silent
        // reorder or dropped seed here is a runtime signing failure, so
        // this pins the contract every call site now depends on.
        assert_eq!(mints[0].as_ref(), &[0xAA; 32]);
        assert_eq!(mints[1].as_ref(), &[0xBB; 32]);
        assert_eq!(bump, [0xCD]);
    }
}
