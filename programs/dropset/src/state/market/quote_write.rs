//! Shared primitives for the two solana-free quote-write kernels —
//! [`stamp_reference_price`](super::stamp_reference_price) behind
//! `SetReferencePrice` and
//! [`write_liquidity_profile`](super::write_liquidity_profile) behind
//! `SetLiquidityProfile`.
//!
//! Both kernels open with the *same* preamble — bounds-check the target
//! sector, compare the signer against its `quote_authority`, then bump the
//! market nonce and arm `FLUSH_BIT` — and diverge only in the payload they
//! write afterwards (three `u32` stores versus a 224-byte profile copy). That
//! preamble lives here once, mirroring how `src/asm/entrypoint.s` shares a
//! single assembly preamble across its two discriminator branches: the Rust
//! reference and the assembly stay aligned in structure, not just in the
//! bytes they land.
//!
//! Everything here is pure byte math over `&mut [u8]` — no Anchor
//! `Context`, no solana system calls — so the exact edge cases (authority
//! mismatch, sector bounds, nonce bump, flush bit) are unit-testable
//! in-process and give the assembly a concrete reference to match.

use core::mem::{offset_of, size_of};

use super::{MarketHeader, ReferencePrice, Vault, FLUSH_BIT};
use crate::asm_offsets::equ;

/// Domain error codes returned by the quote-write kernels. Each equals the
/// `ProgramError::Custom` value anchor-lang-v2's `#[error_code]` produces
/// for the matching [`crate::errors::DropsetError`] variant (its index plus
/// 6000), so the ASM fast path and the Anchor reference build surface the
/// *same* code on the same domain failure. The equality is pinned by
/// the `error_codes_match_dropset` test below.
pub mod err {
    /// Signer is not the target vault's `quote_authority`
    /// (`DropsetError::Unauthorized`).
    pub const UNAUTHORIZED: u32 = 6005;
    /// `vault_idx` is past the live sector count
    /// (`DropsetError::InvalidSectorIndex`).
    pub const INVALID_SECTOR_INDEX: u32 = 6010;
}

// ── Byte offsets within the market account DATA region ───────────────
// The slice handed to a kernel is the account's data region as the
// `Slab<MarketHeader, Vault>` lays it out:
// `[disc:8][MarketHeader][len:u32][pad][Vault; capacity]`. These consts
// reconstruct that framing from the real types (so a field reorder or a
// `Pod*`-width bump moves them in step), and are regression-pinned by the
// asserts below.

/// 8-byte Anchor account discriminator ahead of the header.
const DISC_SIZE: usize = 8;
/// `MarketHeader.nonce` is the header's first field, so it sits at the
/// top of the data region just past the discriminator.
pub(super) const NONCE_OFF: usize = DISC_SIZE + offset_of!(MarketHeader, nonce);
/// Slab's `len: u32`, written immediately after the header.
pub(super) const LEN_OFF: usize = DISC_SIZE + size_of::<MarketHeader>();
/// First `Vault` sector. `Slab` rounds the byte after the `len` field up
/// to `align_of::<Vault>()` — which is 4 (`Vault` embeds `Price`, a
/// `u32`-aligned wrapper), not 1 — so the same `align_up` must be applied
/// here or every sector read lands short by the padding. Computed exactly
/// as `Slab::ITEMS_OFFSET` and cross-checked against it below.
pub(super) const ITEMS_OFF: usize = {
    let after_len = LEN_OFF + size_of::<u32>();
    let align = core::mem::align_of::<Vault>();
    (after_len + align - 1) & !(align - 1)
};
/// One sector's stride.
pub(super) const VAULT_SIZE: usize = size_of::<Vault>();

// ── Offsets within a single `Vault` sector ──────────────────────────
// Only the fields the shared preamble touches live here; each kernel owns
// the offsets of the payload it alone writes (`reference_price.price` /
// `quote_slot` / `quote_unix`, or `profile`).
pub(super) const VAULT_QUOTE_AUTHORITY_OFF: usize = offset_of!(Vault, quote_authority);
pub(super) const VAULT_REFERENCE_PRICE_OFF: usize = offset_of!(Vault, reference_price);
pub(super) const RP_STAMP_OFF: usize = offset_of!(ReferencePrice, stamp);

// Regression guards on the reconstructed framing. `layout.rs` already
// pins the struct internals (`Vault` size / field offsets); these pin the
// Slab framing the kernels and the ASM both hardcode, so a header-size or
// alignment change breaks the build here rather than silently
// mis-stamping.
//
// They compare against the **assembly's own `.equ` table** rather than
// against concrete literals, which is what they used to carry. Those
// literals were a third hand-typed copy of numbers `entrypoint.s`
// hardcodes and `tests/asm_parity.rs` re-typed again, with no mechanical
// link between the three: a layout change failed the copies one at a time,
// and updating each in turn restored a green build while leaving the
// assembly aimed at the old offset. `build.rs` now lifts the assembly's
// table into `crate::asm_offsets::equ`, so the comparison below is
// directly against what the assembly stores through — one hand-written
// source, and a mismatch fails the build on whichever side moved. See
// `src/asm_offsets.rs`.
const _: () = assert!(NONCE_OFF as u64 == equ::MARKET_NONCE_OFF - equ::MARKET_DATA_OFF);
const _: () = assert!(LEN_OFF as u64 == equ::MARKET_LEN_OFF - equ::MARKET_DATA_OFF);
const _: () = assert!(ITEMS_OFF as u64 == equ::SLAB_ITEMS_OFF);
// Authoritative pin: `Slab::space_for(0)` *is* the slab's `ITEMS_OFFSET`,
// so this guarantees the kernels' sector base can never drift from the
// real on-chain layout (a header-size or `Vault`-alignment change breaks
// the build here).
const _: () = assert!(ITEMS_OFF == crate::state::Market::space_for(0));
const _: () = assert!(VAULT_SIZE as u64 == equ::VAULT_SIZE);
const _: () = assert!(VAULT_QUOTE_AUTHORITY_OFF as u64 == equ::VAULT_QUOTE_AUTHORITY_OFF);
const _: () = assert!((VAULT_REFERENCE_PRICE_OFF + RP_STAMP_OFF) as u64 == equ::RP_STAMP_OFF);

/// Bounds-check `vault_idx` and authorize the write, returning the target
/// sector's byte offset within `data` on success.
///
/// The single domain guard both quote-write paths apply: the signer must be
/// the target vault's `quote_authority`. Deliberately *not* checked (on
/// either path, matching the assembly):
///
/// * **Occupancy** — a write to a free-listed sector is **inert**, so it
///   needs no guard. Note the compare does *not* reject it: `reclaim_sector`
///   zeroes only `leader` (the emptiness marker `Vault::is_occupied` reads),
///   so a freed sector keeps its former `quote_authority` until
///   `allocate_sector` re-zeroes the whole struct on reuse — meaning that
///   ex-authority's compare still passes. What makes it harmless is the
///   *blast radius*: a quote write touches only `market.nonce`, the
///   sector's `reference_price` and its `profile`, never the `next` / `prev`
///   links (offsets 0 / 4) that thread the free list, and matching walks
///   the active DLL only, so a free sector never enters the book. Pinned by
///   the `write_to_a_reclaimed_sector_is_inert` test below.
/// * **`frozen`** — the freeze is enforced at match time (`swap` skips
///   frozen vaults), and re-quoting one is inert, so both kernels stay
///   minimal by omitting it (see `freeze_vault.rs`).
/// * **Price and ladder values** — neither kernel bounds *what* the
///   leader publishes. `stamp_reference_price` stores `price` raw and
///   `write_liquidity_profile` copies the ladder verbatim, so a leader
///   may anchor anywhere in the representable `Price` range (~1e-16 to
///   ~1e16) and offset any level by up to `u32::MAX` ppm. The offset's
///   *effect* saturates — asks top out near 4295 times the reference,
///   and a bid at or above `1e6` ppm floors to `Price::ZERO` and leaves
///   the book — but the anchor does not, so neither side's distance from
///   the market is bounded, and in ratio terms bids reach the farther of
///   the two. Deliberate, and **not fixable by a write-time band**:
///   `Level::price_offset` is measured from a reference price the *same*
///   leader chooses, so
///   bounding the offset bounds the ladder's *shape*, not its distance
///   from the market — a leader wanting to quote far out just stamps a
///   far-out reference instead. A band that bit would need an external
///   price truth, which this design refuses on purpose (the leader's
///   stamp *is* the price datum). What bounds the loss is taker consent,
///   downstream: the per-level limit filter, the price-time walk that
///   sorts a far-out level to the tail of the book, and the `min_out`
///   soft-revert. Hence the invariant binding anything that consumes
///   value at a level price — **never derive a bound from the level's
///   own price, only from a price the taker demonstrably accepted**. The
///   exact-in walk obeys it by giving `LegFill::Exhausted` no residue
///   field at all (see `swap.rs`): that arm fires when the taker
///   receives nothing *at that level*, so its price is not one they
///   accepted and `min_out` is no defense, since residue never reduces
///   output.
///
/// Runs before any mutation, so a rejected call leaves `data` untouched.
#[inline(always)]
pub(super) fn authorize_quote_write(
    data: &[u8],
    vault_idx: u32,
    signer_key: &[u8; 32],
) -> Result<usize, u32> {
    let idx = vault_idx as usize;

    // Bounds: accept only when `idx` is within the live sector count,
    // which is `min(len, capacity)` — matching `Slab::as_mut_slice`'s
    // `effective_len` so the kernels, the typed accessor, and the ASM all
    // reject the same indices. Split into the two `min` legs to avoid a
    // division (`idx < capacity` ⇔ `ITEMS_OFF + (idx+1)*VAULT_SIZE <=
    // data.len()`).
    let len = read_u32(data, LEN_OFF) as usize;
    let vault_off = ITEMS_OFF + idx * VAULT_SIZE;
    if idx >= len || vault_off + VAULT_SIZE > data.len() {
        return Err(err::INVALID_SECTOR_INDEX);
    }

    let auth_off = vault_off + VAULT_QUOTE_AUTHORITY_OFF;
    if &data[auth_off..auth_off + 32] != signer_key {
        return Err(err::UNAUTHORIZED);
    }

    Ok(vault_off)
}

/// Bump `market.nonce` and stamp the *old* nonce OR'd with `FLUSH_BIT`
/// onto the sector at `vault_off`, so the next taker re-materializes
/// `remaining` from the vault's `LiquidityProfile`.
///
/// `wrapping_add` rather than a checked add: the nonce is a u64 monotonic
/// counter that can't overflow in any realistic horizon, and the ASM path
/// can't cheaply raise a custom overflow error — wrapping keeps the two
/// implementations identical. Both quote-write kernels wrap for that
/// reason; `swap` is the one nonce-bumping path that `checked_add`s and
/// rejects overflow.
///
/// Leaves `reference_price.price` and `quote_slot` alone — each kernel
/// writes its own payload after this.
#[inline(always)]
pub(super) fn bump_nonce_and_arm_flush(data: &mut [u8], vault_off: usize) {
    let nonce = read_u64(data, NONCE_OFF);
    write_u64(data, NONCE_OFF, nonce.wrapping_add(1));
    write_u64(
        data,
        vault_off + VAULT_REFERENCE_PRICE_OFF + RP_STAMP_OFF,
        nonce | FLUSH_BIT,
    );
}

// Little-endian, alignment-free accessors. The on-chain layout is
// alignment-1 `Pod` wrappers stored little-endian, and `data` is a raw
// byte region with no alignment guarantee, so every read / write goes
// through `from_le_bytes` / `to_le_bytes` on a copy (never a `*const u64`
// cast). Callers have already bounds-checked the sector, so the slices are
// in range.
#[inline(always)]
pub(super) fn read_u32(data: &[u8], off: usize) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&data[off..off + 4]);
    u32::from_le_bytes(buf)
}

#[inline(always)]
pub(super) fn read_u64(data: &[u8], off: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[off..off + 8]);
    u64::from_le_bytes(buf)
}

#[inline(always)]
pub(super) fn write_u32(data: &mut [u8], off: usize, value: u32) {
    data[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

#[inline(always)]
pub(super) fn write_u64(data: &mut [u8], off: usize, value: u64) {
    data[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

/// Synthetic market buffers shared by both kernels' unit tests — the
/// raw-bytes counterpart to [`super::test_support`], which builds a real
/// `Slab`-backed account. The kernels take `&mut [u8]`, so most of their
/// cases want a hand-framed buffer; the two that must cross-check against
/// the typed layout use `test_support` directly. Sector count is reused
/// from there so the two fixtures can't drift apart.
#[cfg(test)]
pub(super) mod test_buf {
    /// Re-exported so a `test_buf::*` import carries the sector count too.
    pub(crate) use super::super::test_support::SECTORS;
    use super::*;

    /// The `quote_authority` the fixtures stamp, i.e. the authorized signer.
    pub const AUTH: [u8; 32] = [0x11; 32];
    /// Any other signer — rejected with [`err::UNAUTHORIZED`].
    pub const OTHER: [u8; 32] = [0x22; 32];

    /// Build a market data region with [`SECTORS`] zeroed sectors, `len`
    /// set, and sector `auth_idx`'s `quote_authority` = [`AUTH`]. Mirrors
    /// the `Slab` framing the on-chain account uses.
    pub fn market_buf(auth_idx: usize) -> Vec<u8> {
        let mut data = vec![0u8; ITEMS_OFF + SECTORS as usize * VAULT_SIZE];
        data[LEN_OFF..LEN_OFF + 4].copy_from_slice(&SECTORS.to_le_bytes());
        let auth_off = ITEMS_OFF + auth_idx * VAULT_SIZE + VAULT_QUOTE_AUTHORITY_OFF;
        data[auth_off..auth_off + 32].copy_from_slice(&AUTH);
        data
    }
}

#[cfg(test)]
mod tests {
    use super::test_buf::*;
    use super::*;
    use crate::errors::DropsetError;

    #[test]
    fn error_codes_match_dropset() {
        // anchor-lang-v2 `#[error_code]` maps a fieldless variant to
        // `Custom(index + 6000)`; pin the kernels' domain codes to that so
        // ASM and Anchor can't drift apart.
        const OFFSET: u32 = 6000;
        assert_eq!(
            err::UNAUTHORIZED,
            DropsetError::Unauthorized as u32 + OFFSET
        );
        assert_eq!(
            err::INVALID_SECTOR_INDEX,
            DropsetError::InvalidSectorIndex as u32 + OFFSET
        );
    }

    #[test]
    fn authorizes_the_quote_authority() {
        let data = market_buf(2);
        assert_eq!(
            authorize_quote_write(&data, 2, &AUTH),
            Ok(ITEMS_OFF + 2 * VAULT_SIZE)
        );
    }

    #[test]
    fn wrong_authority_rejected() {
        let data = market_buf(1);
        assert_eq!(
            authorize_quote_write(&data, 1, &OTHER),
            Err(err::UNAUTHORIZED)
        );
        // A sector whose `quote_authority` was never populated (all zero) is
        // rejected by the same compare. This is *not* the reclaimed-sector
        // case — see `write_to_a_reclaimed_sector_is_inert`, which covers a
        // sector that keeps a live authority on the free list.
        assert_eq!(
            authorize_quote_write(&data, 0, &AUTH),
            Err(err::UNAUTHORIZED)
        );
    }

    #[test]
    fn write_to_a_reclaimed_sector_is_inert() {
        // The occupancy guard both quote writes dropped is safe because the
        // write is inert on a freed sector — *not* because the authority
        // compare rejects one. `reclaim_sector` zeroes only `leader`, so the
        // ex-authority still passes the compare; what must hold is that the
        // write can't corrupt the free list or re-enter the book.
        use super::super::test_support::{load_market, setup};
        use super::super::{DllList, VaultDll};

        let buf = setup();
        {
            let mut market = load_market(&buf);
            // Sector 1 goes live with a real quote authority, then is
            // reclaimed onto the free list.
            market.as_mut_slice()[1].quote_authority = AUTH.into();
            market.as_mut_slice()[1].leader = [0x33; 32].into();
            market.link_head(DllList::Active, 1).expect("link active");
            market.reclaim_sector(1).expect("reclaim");
            // Precondition for this test to mean anything: the authority
            // survived the reclaim while `leader` was zeroed.
            assert!(
                !market.as_slice()[1].is_occupied(),
                "reclaim zeroes `leader`"
            );
            assert_eq!(
                market.as_slice()[1].quote_authority.to_bytes(),
                AUTH,
                "reclaim does NOT zero `quote_authority` — the premise here"
            );
        }
        // Read the post-reclaim bytes directly; `load_market` resets the
        // list heads on every call, so the raw buffer — not a second load —
        // is what preserves the free-list state across the write.
        let before = buf.read_data().to_vec();

        // The ex-authority's write is therefore *authorized*: the compare
        // passes on a sector that is no longer in use.
        let mut data = before.clone();
        let vault_off = authorize_quote_write(&data, 1, &AUTH)
            .expect("a reclaimed sector keeps its quote_authority, so this is authorized");
        bump_nonce_and_arm_flush(&mut data, vault_off);

        // …and inert. Bound the blast radius by byte index: only the nonce
        // and this sector's `reference_price.stamp` may move. That covers the
        // free-list linkage (`next` / `prev` at sector offsets 0 / 4) and the
        // header's `free_head` without naming them individually, so a future
        // payload that strayed into either would fail here.
        let nonce = NONCE_OFF..NONCE_OFF + 8;
        let stamp_off = vault_off + VAULT_REFERENCE_PRICE_OFF + RP_STAMP_OFF;
        let stamp = stamp_off..stamp_off + 8;
        for (i, (b, a)) in before.iter().zip(data.iter()).enumerate() {
            assert!(
                b == a || nonce.contains(&i) || stamp.contains(&i),
                "byte {i} changed outside the nonce / stamp of the freed sector"
            );
        }
    }

    #[test]
    fn out_of_range_index_rejected() {
        let data = market_buf(0);
        // `SECTORS` is one past the last live sector.
        assert_eq!(
            authorize_quote_write(&data, SECTORS, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        // The null-sector sentinel is the worst case.
        assert_eq!(
            authorize_quote_write(&data, u32::MAX, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
    }

    #[test]
    fn index_within_len_but_past_capacity_rejected() {
        // `len` claims more sectors than the buffer physically holds (the
        // post-external-resize edge `Slab::effective_len` guards). The
        // capacity leg must still reject, matching `min(len, capacity)`.
        let mut data = market_buf(0);
        write_u32(&mut data, LEN_OFF, SECTORS + 2);
        assert_eq!(
            authorize_quote_write(&data, SECTORS, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
    }

    #[test]
    fn arms_flush_with_the_old_nonce() {
        let mut data = market_buf(1);
        write_u64(&mut data, NONCE_OFF, 41);
        let vault_off = ITEMS_OFF + VAULT_SIZE;
        bump_nonce_and_arm_flush(&mut data, vault_off);
        assert_eq!(read_u64(&data, NONCE_OFF), 42, "nonce advanced by one");
        assert_eq!(
            read_u64(&data, vault_off + VAULT_REFERENCE_PRICE_OFF + RP_STAMP_OFF),
            41 | FLUSH_BIT,
            "stamp carries the OLD nonce OR'd with the flush bit"
        );
    }

    #[test]
    fn nonce_wraps_at_u64_max() {
        let mut data = market_buf(0);
        write_u64(&mut data, NONCE_OFF, u64::MAX);
        bump_nonce_and_arm_flush(&mut data, ITEMS_OFF);
        // Old nonce (all ones) already has the flush bit set.
        assert_eq!(
            read_u64(&data, ITEMS_OFF + VAULT_REFERENCE_PRICE_OFF + RP_STAMP_OFF),
            u64::MAX
        );
        assert_eq!(read_u64(&data, NONCE_OFF), 0);
    }
}
