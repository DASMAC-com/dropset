// cspell:word misdecoding
//! Zero-copy mirror of the on-chain market account layout.
//!
//! The program stores a market as `Slab<MarketHeader, Vault>`: an 8-byte
//! Anchor account discriminator, then a fixed `MarketHeader`, then a
//! `u32` slab length, then a tail of `Vault` sectors (see
//! programs/dropset/src/state/market/layout.rs and architecture.md § Storage
//! layout). The `Vault` slab is **opaque to the IDL**, so the generated
//! client can't decode it — this module mirrors the layout so the
//! matching simulator (and any depth/book renderer) can.
//!
//! Every on-chain field is alignment-1 (the program uses `PodU64`-style
//! byte wrappers so the slab casts directly from account bytes). We
//! mirror that with little-endian wrappers, keeping each struct
//! `repr(C)`, align-1, and padding-free — so `bytemuck` casts the raw
//! account bytes with no copy and the size asserts below catch any drift
//! from the on-chain definition.

use bytemuck::{Pod, Zeroable};

use crate::price::Price;

pub const N_LEVELS: usize = 8;
/// Sentinel for sector-index pointers (`head`, `next`, `prev`).
pub const NULL_SECTOR: u32 = u32::MAX;
/// Flush flag OR'd onto `ReferencePrice::stamp`.
pub const FLUSH_BIT: u64 = 1u64 << 63;
pub const PPM: u64 = 1_000_000;
pub const BPS: u64 = 10_000;
/// Anchor account discriminator length.
pub const ACCOUNT_DISCRIMINATOR_LEN: usize = 8;
/// On-chain `align_of::<Vault>()`. The program's `Vault` embeds `Price`
/// (a `#[repr(transparent)] u32`, align 4), so the slab aligns the first
/// sector to 4 — even though every field in this mirror is alignment-1.
/// The first sector therefore starts at `align_up(8 +
/// size_of::<MarketHeader>() + 4, 4)`, not flush against the slab length.
/// Pinned by programs/dropset/tests/sdk_conformance.rs.
pub const VAULT_ALIGN: usize = 4;

// ── Little-endian, alignment-1 integer wrappers ──────────────────────

macro_rules! le_int {
    ($name:ident, $int:ty, $n:literal) => {
        #[repr(transparent)]
        #[derive(Copy, Clone, Default, Pod, Zeroable)]
        pub struct $name([u8; $n]);
        impl $name {
            #[inline(always)]
            pub fn get(self) -> $int {
                <$int>::from_le_bytes(self.0)
            }
        }
        impl From<$int> for $name {
            #[inline(always)]
            fn from(v: $int) -> Self {
                Self(v.to_le_bytes())
            }
        }
        impl core::fmt::Debug for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}", self.get())
            }
        }
    };
}

le_int!(LeU16, u16, 2);
le_int!(LeU32, u32, 4);
le_int!(LeU64, u64, 8);

// ── Layout structs (mirror programs/dropset/src/state/market/layout.rs) ──

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct FeeConfig {
    pub mint: [u8; 32],
    pub token_program: [u8; 32],
    pub atoms: LeU64,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ReferencePrice {
    pub stamp: LeU64,
    pub price: LeU32,
    pub quote_slot: LeU32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Level {
    pub price_offset: LeU32,
    pub size_bps: LeU16,
    pub expiry_offset: LeU32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LiquidityProfile {
    pub bids: [Level; N_LEVELS],
    pub asks: [Level; N_LEVELS],
}

/// A per-side `Σ size_bps` that crosses [`BPS`] — the two totals, for the
/// caller to report. Returned by [`LiquidityProfile::validate_size_sums`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SizeSumViolation {
    pub bid_sum: u32,
    pub ask_sum: u32,
}

impl LiquidityProfile {
    /// The per-side `(bids, asks)` `Σ size_bps`. At `N_LEVELS = 8` the
    /// widest sum is `8 × u16::MAX = 524_280`, well inside `u32`.
    pub fn size_bps_sums(&self) -> (u32, u32) {
        let bid = self.bids.iter().map(|l| l.size_bps.get() as u32).sum();
        let ask = self.asks.iter().map(|l| l.size_bps.get() as u32).sum();
        (bid, ask)
    }

    /// The canonical per-side `Σ size_bps ≤ BPS` check — the same threshold
    /// the on-chain matcher applies at flush time to decide whether a side
    /// is materialized or thrown out. Every off-chain profile builder routes
    /// through this before submitting so an honest client never emits a side
    /// the engine would silently skip (a no-fill). `Σ == BPS` is valid; only
    /// `> BPS` fails.
    pub fn validate_size_sums(&self) -> Result<(), SizeSumViolation> {
        let (bid_sum, ask_sum) = self.size_bps_sums();
        if bid_sum > BPS as u32 || ask_sum > BPS as u32 {
            return Err(SizeSumViolation { bid_sum, ask_sum });
        }
        Ok(())
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Position {
    pub price: LeU32,
    pub size: LeU64,
    pub expires_at: LeU32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Remaining {
    pub bids: [Position; N_LEVELS],
    pub asks: [Position; N_LEVELS],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct MarketHeader {
    pub nonce: LeU64,
    pub head: LeU32,
    pub tombstone_head: LeU32,
    pub free_head: LeU32,
    pub active_count: LeU32,
    pub outstanding_vault_depositors: LeU32,
    pub fee_config: FeeConfig,
    pub taker_fee: LeU16,
    pub default_min_leader_share: LeU32,
    pub base_mint: [u8; 32],
    pub quote_mint: [u8; 32],
    pub base_treasury: [u8; 32],
    pub quote_treasury: [u8; 32],
    pub bump: u8,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vault {
    pub next: LeU32,
    pub prev: LeU32,
    pub leader: [u8; 32],
    pub quote_authority: [u8; 32],
    pub reference_price: ReferencePrice,
    pub base_atoms: LeU64,
    pub quote_atoms: LeU64,
    pub total_shares: LeU64,
    pub leader_shares: LeU64,
    pub hwm: LeU64,
    pub perf_fee_rate: LeU32,
    pub min_leader_share: LeU32,
    pub frozen: u8,
    pub allow_outside_depositors: u8,
    pub outside_deposits_approved: u8,
    pub tombstoned: u8,
    pub _reserved: [u8; 4],
    pub profile: LiquidityProfile,
    pub remaining: Remaining,
}

// Size guards — mirror the on-chain `const _: assert!`s. Any drift in
// the program layout (without regenerating against a fresh IDL + updating
// this mirror) breaks the SDK build here rather than silently
// misdecoding the slab.
const _: () = assert!(core::mem::size_of::<MarketHeader>() == 235);
const _: () = assert!(core::mem::size_of::<Vault>() == 560);
// Sectors stay aligned across the slab: stride must be a multiple of the
// on-chain Vault alignment (see VAULT_ALIGN / MarketView::load).
const _: () = assert!(core::mem::size_of::<Vault>().is_multiple_of(VAULT_ALIGN));
const _: () = assert!(core::mem::size_of::<LiquidityProfile>() == 2 * N_LEVELS * 10);
const _: () = assert!(core::mem::size_of::<Remaining>() == 2 * N_LEVELS * 16);

impl ReferencePrice {
    /// Decoded reference price (raw bits -> `Price`).
    #[inline]
    pub fn price(&self) -> Price {
        Price::from_bits(self.price.get())
    }
    /// `stamp` with `FLUSH_BIT` masked off — the price-time nonce.
    #[inline]
    pub fn nonce(&self) -> u64 {
        self.stamp.get() & !FLUSH_BIT
    }
    /// Whether a flush is armed (set by `set_reference_price` /
    /// `set_liquidity_profile`, cleared by the first matching taker).
    #[inline]
    pub fn flush_armed(&self) -> bool {
        self.stamp.get() & FLUSH_BIT != 0
    }
}

impl Vault {
    /// A vault on the free list has a zeroed leader.
    #[inline]
    pub fn is_free(&self) -> bool {
        self.leader == [0u8; 32]
    }
}

/// Zero-copy view over a decoded market account.
#[derive(Debug)]
pub struct MarketView<'a> {
    pub header: &'a MarketHeader,
    sectors: &'a [Vault],
}

/// Error decoding a market account's raw bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// Buffer shorter than discriminator + header + slab length.
    TooSmall,
    /// The slab `len` implies more sectors than the buffer holds.
    SectorOverflow,
    /// `bytemuck` could not cast the slab tail (should not happen at align-1).
    Cast,
}

impl<'a> MarketView<'a> {
    /// Decode a market account's full data buffer (including the 8-byte
    /// discriminator) into a zero-copy view.
    pub fn load(data: &'a [u8]) -> Result<Self, LayoutError> {
        let hdr_len = core::mem::size_of::<MarketHeader>();
        let len_at = ACCOUNT_DISCRIMINATOR_LEN + hdr_len;
        if data.len() < len_at + 4 {
            return Err(LayoutError::TooSmall);
        }
        let header: &MarketHeader =
            bytemuck::try_from_bytes(&data[ACCOUNT_DISCRIMINATOR_LEN..len_at])
                .map_err(|_| LayoutError::Cast)?;
        let len = u32::from_le_bytes(data[len_at..len_at + 4].try_into().unwrap()) as usize;
        // The slab aligns the first sector to the on-chain Vault align
        // (see VAULT_ALIGN); subsequent sectors are size_of::<Vault>()
        // apart, which is a multiple of VAULT_ALIGN so they stay aligned.
        let items_start = (len_at + 4 + VAULT_ALIGN - 1) & !(VAULT_ALIGN - 1);
        let need = len
            .checked_mul(core::mem::size_of::<Vault>())
            .ok_or(LayoutError::SectorOverflow)?;
        let end = items_start
            .checked_add(need)
            .ok_or(LayoutError::SectorOverflow)?;
        if data.len() < end {
            return Err(LayoutError::SectorOverflow);
        }
        let sectors: &[Vault] =
            bytemuck::try_cast_slice(&data[items_start..end]).map_err(|_| LayoutError::Cast)?;
        Ok(Self { header, sectors })
    }

    /// All sectors in the slab (active, tombstoned, and free).
    #[inline]
    pub fn sectors(&self) -> &'a [Vault] {
        self.sectors
    }

    /// Walk the active doubly-linked list from `header.head`, yielding
    /// `(sector_index, &Vault)`. Bounded by the slab length so a corrupt
    /// `next` pointer can't loop forever — the same guard the on-chain
    /// matcher uses.
    pub fn active_vaults(&self) -> impl Iterator<Item = (u32, &'a Vault)> {
        let sectors = self.sectors;
        let mut cur = self.header.head.get();
        let mut steps = sectors.len();
        core::iter::from_fn(move || {
            if cur == NULL_SECTOR || steps == 0 {
                return None;
            }
            steps -= 1;
            let idx = cur as usize;
            let v = sectors.get(idx)?;
            cur = v.next.get();
            Some((idx as u32, v))
        })
    }

    /// Whether the active DLL is corrupt: a `next` pointer that runs out
    /// of bounds, or a cycle / over-length chain that exhausts the
    /// slab-length step budget before reaching [`NULL_SECTOR`]. Walks the
    /// list exactly as [`active_vaults`](Self::active_vaults) does, but
    /// *reports* the truncation that iterator hides — where `active_vaults`
    /// silently ends on a bad pointer (and a caller would quote whatever
    /// levels it had already collected), this surfaces it as a boolean.
    ///
    /// Mirrors the on-chain engine's two `CorruptVaultList` guards in
    /// `swap.rs` (the `steps_remaining > 0` budget and the in-bounds
    /// `(cur as usize) < len` check), so a caller can refuse to act on a
    /// list the engine would abort the whole `swap` over — see
    /// `crate::matching::simulate_swap`. Only fires on account bytes the
    /// program never wrote: the DLL ops keep the active list acyclic and
    /// in-bounds, so a cycle or dangling pointer requires corruption.
    pub fn active_dll_is_corrupt(&self) -> bool {
        let sectors = self.sectors;
        let mut cur = self.header.head.get();
        let mut steps = sectors.len();
        while cur != NULL_SECTOR {
            if steps == 0 {
                // Step budget spent before NULL: a cycle or a chain longer
                // than the slab. Engine: `require!(steps_remaining > 0)`.
                return true;
            }
            steps -= 1;
            if cur as usize >= sectors.len() {
                // `next` points past the slab tail. Engine:
                // `require!((cur as usize) < len)`.
                return true;
            }
            cur = sectors[cur as usize].next.get();
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn profile_with(bid_bps: &[u16], ask_bps: &[u16]) -> LiquidityProfile {
        let mut p = LiquidityProfile::zeroed();
        for (i, &b) in bid_bps.iter().enumerate() {
            p.bids[i].size_bps = b.into();
        }
        for (i, &a) in ask_bps.iter().enumerate() {
            p.asks[i].size_bps = a.into();
        }
        p
    }

    #[test]
    fn size_sums_at_and_below_bps_are_valid() {
        // `Σ == BPS` is valid — only strictly over the cap fails.
        assert!(profile_with(&[9_999], &[0]).validate_size_sums().is_ok());
        assert!(profile_with(&[6_000, 4_000], &[10_000])
            .validate_size_sums()
            .is_ok());
    }

    #[test]
    fn size_sums_over_bps_are_rejected_per_side() {
        // Bid side one over the cap; ask side clean.
        let err = profile_with(&[6_000, 4_001], &[0])
            .validate_size_sums()
            .unwrap_err();
        assert_eq!(err.bid_sum, 10_001);
        assert_eq!(err.ask_sum, 0);
        // Ask side over; bid side clean. A single level past BPS trips it too.
        let err = profile_with(&[0], &[10_001])
            .validate_size_sums()
            .unwrap_err();
        assert_eq!(err.ask_sum, 10_001);
        assert_eq!(err.bid_sum, 0);
    }

    #[test]
    fn size_bps_sums_reports_both_sides() {
        assert_eq!(
            profile_with(&[1_000, 2_000], &[3_000]).size_bps_sums(),
            (3_000, 3_000)
        );
    }
}
