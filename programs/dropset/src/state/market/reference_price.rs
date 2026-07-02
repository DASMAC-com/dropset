//! `stamp_reference_price` — the solana-free kernel behind the
//! `SetReferencePrice` leader hot path.
//!
//! One function, operating directly on a market account's data bytes, is
//! the single source of truth for the leader-price stamp: the non-asm
//! Anchor handler calls it, and the hand-written sBPF `entrypoint.s`
//! mirrors it byte-for-byte (see the architecture spec's
//! **SetReferencePrice**). Keeping the logic here — pure byte math over
//! `&mut [u8]`, no Anchor `Context`, no solana system calls — lets the exact
//! edge cases (authority mismatch, sector bounds, nonce bump, flush bit,
//! price / slot packing) be unit-tested in-process, and gives the ASM a
//! concrete reference to match.

use core::mem::{offset_of, size_of};

use super::{MarketHeader, ReferencePrice, Vault, FLUSH_BIT};

/// Domain error codes returned by [`stamp_reference_price`]. Each equals
/// the `ProgramError::Custom` value anchor-lang-v2's `#[error_code]`
/// produces for the matching [`crate::errors::DropsetError`] variant
/// (variant index + 6000), so the ASM fast path and the Anchor reference
/// build surface the *same* code on the same domain failure. The
/// equality is pinned by [`tests::error_codes_match_dropset`].
pub mod err {
    /// Signer is not the target vault's `quote_authority`
    /// (`DropsetError::Unauthorized`).
    pub const UNAUTHORIZED: u32 = 6005;
    /// `vault_idx` is past the live sector count
    /// (`DropsetError::InvalidSectorIndex`).
    pub const INVALID_SECTOR_INDEX: u32 = 6010;
}

// ── Byte offsets within the market account DATA region ───────────────
// The slice handed to the kernel is the account's data region as the
// `Slab<MarketHeader, Vault>` lays it out:
// `[disc:8][MarketHeader][len:u32][pad][Vault; capacity]`. These consts
// reconstruct that framing from the real types (so a field reorder or a
// `Pod*`-width bump moves them in step), and are regression-pinned by the
// asserts below.

/// 8-byte Anchor account discriminator ahead of the header.
const DISC_SIZE: usize = 8;
/// `MarketHeader.nonce` is the header's first field, so it sits at the
/// top of the data region just past the discriminator.
const NONCE_OFF: usize = DISC_SIZE + offset_of!(MarketHeader, nonce);
/// Slab's `len: u32`, written immediately after the header.
const LEN_OFF: usize = DISC_SIZE + size_of::<MarketHeader>();
/// First `Vault` sector. `Slab` rounds the byte after the `len` field up
/// to `align_of::<Vault>()` — which is 4 (`Vault` embeds `Price`, a
/// `u32`-aligned wrapper), not 1 — so the same `align_up` must be applied
/// here or every sector read lands short by the padding. Computed exactly
/// as `Slab::ITEMS_OFFSET` and cross-checked against it below.
const ITEMS_OFF: usize = {
    let after_len = LEN_OFF + size_of::<u32>();
    let align = core::mem::align_of::<Vault>();
    (after_len + align - 1) & !(align - 1)
};
/// One sector's stride.
const VAULT_SIZE: usize = size_of::<Vault>();

// ── Offsets within a single `Vault` sector ──────────────────────────
const VAULT_QUOTE_AUTHORITY_OFF: usize = offset_of!(Vault, quote_authority);
const VAULT_REFERENCE_PRICE_OFF: usize = offset_of!(Vault, reference_price);
const RP_STAMP_OFF: usize = offset_of!(ReferencePrice, stamp);
const RP_PRICE_OFF: usize = offset_of!(ReferencePrice, price);
const RP_QUOTE_SLOT_OFF: usize = offset_of!(ReferencePrice, quote_slot);

// Regression guards on the reconstructed framing. `layout.rs` already
// pins the struct internals (`Vault` size / field offsets); these pin the
// Slab framing the kernel and the ASM both hardcode, so a header-size or
// alignment change breaks the build here rather than silently
// mis-stamping. Kept as concrete literals (not just the derivations
// above) so a change to either side is caught.
const _: () = assert!(NONCE_OFF == 8);
const _: () = assert!(LEN_OFF == 243);
const _: () = assert!(ITEMS_OFF == 248);
// Authoritative pin: `Slab::space_for(0)` *is* the slab's `ITEMS_OFFSET`,
// so this guarantees the kernel's sector base can never drift from the
// real on-chain layout (a header-size or `Vault`-alignment change breaks
// the build here).
const _: () = assert!(ITEMS_OFF == crate::state::Market::space_for(0));
const _: () = assert!(VAULT_SIZE == 560);
const _: () = assert!(VAULT_QUOTE_AUTHORITY_OFF == 40);
const _: () = assert!(VAULT_REFERENCE_PRICE_OFF == 72);
const _: () = assert!(RP_STAMP_OFF == 0);
const _: () = assert!(RP_PRICE_OFF == 8);
const _: () = assert!(RP_QUOTE_SLOT_OFF == 12);

/// Stamp `(price_bits, quote_slot)` onto vault `vault_idx`'s reference
/// price, arm the flush bit, and bump the market nonce — the entire
/// steady-state leader hot path, expressed as pure byte math.
///
/// `data` is the market account's full data region (discriminator
/// included). `signer_key` is the transaction signer's pubkey; the one
/// domain guard is that it equals the target vault's `quote_authority`
/// (per the architecture spec's **SetReferencePrice**, price / slot
/// values are stored raw — matching skips an invalid price, so no
/// write-time validation is needed).
///
/// On any domain failure it returns an [`err`] code with `data`
/// unmodified: every check runs before the nonce is bumped, so a rejected
/// call never advances market state.
#[inline]
pub fn stamp_reference_price(
    data: &mut [u8],
    vault_idx: u32,
    price_bits: u32,
    quote_slot: u32,
    signer_key: &[u8; 32],
) -> Result<(), u32> {
    let idx = vault_idx as usize;

    // Bounds: accept only when `idx` is within the live sector count,
    // which is `min(len, capacity)` — matching `Slab::as_mut_slice`'s
    // `effective_len` so the kernel, the typed accessor, and the ASM all
    // reject the same indices. Split into the two `min` legs to avoid a
    // division (`idx < capacity` ⇔ `ITEMS_OFF + (idx+1)*VAULT_SIZE <=
    // data.len()`).
    let len = read_u32(data, LEN_OFF) as usize;
    let vault_off = ITEMS_OFF + idx * VAULT_SIZE;
    if idx >= len || vault_off + VAULT_SIZE > data.len() {
        return Err(err::INVALID_SECTOR_INDEX);
    }

    // The only domain guard: signer must be the vault's quote authority.
    let auth_off = vault_off + VAULT_QUOTE_AUTHORITY_OFF;
    if &data[auth_off..auth_off + 32] != signer_key {
        return Err(err::UNAUTHORIZED);
    }

    // Bump the nonce; the stamp carries the OLD nonce OR'd with the flush
    // bit, so the next taker re-materializes `remaining` from the
    // (unchanged) `LiquidityProfile`. `wrapping_add` rather than a checked
    // add: the nonce is a u64 monotonic counter that can't overflow in any
    // realistic horizon, and the ASM path can't cheaply raise a custom
    // overflow error — wrapping keeps the two implementations identical.
    let nonce = read_u64(data, NONCE_OFF);
    let stamp = nonce | FLUSH_BIT;
    write_u64(data, NONCE_OFF, nonce.wrapping_add(1));

    // Stamp the reference price: `stamp` (u64), then the packed
    // `(price, quote_slot)` as two adjacent u32s.
    let rp_off = vault_off + VAULT_REFERENCE_PRICE_OFF;
    write_u64(data, rp_off + RP_STAMP_OFF, stamp);
    write_u32(data, rp_off + RP_PRICE_OFF, price_bits);
    write_u32(data, rp_off + RP_QUOTE_SLOT_OFF, quote_slot);
    Ok(())
}

// Little-endian, alignment-free accessors. The on-chain layout is
// alignment-1 `Pod` wrappers stored little-endian, and `data` is a raw
// byte region with no alignment guarantee, so every read / write goes
// through `from_le_bytes` / `to_le_bytes` on a copy (never a `*const u64`
// cast). Callers have already bounds-checked the sector, so the slices are
// in range.
#[inline(always)]
fn read_u32(data: &[u8], off: usize) -> u32 {
    let mut buf = [0u8; 4];
    buf.copy_from_slice(&data[off..off + 4]);
    u32::from_le_bytes(buf)
}

#[inline(always)]
fn read_u64(data: &[u8], off: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&data[off..off + 8]);
    u64::from_le_bytes(buf)
}

#[inline(always)]
fn write_u32(data: &mut [u8], off: usize, value: u32) {
    data[off..off + 4].copy_from_slice(&value.to_le_bytes());
}

#[inline(always)]
fn write_u64(data: &mut [u8], off: usize, value: u64) {
    data[off..off + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::DropsetError;

    const AUTH: [u8; 32] = [0x11; 32];
    const OTHER: [u8; 32] = [0x22; 32];
    const SECTORS: usize = 4;

    /// Build a market data region with `SECTORS` zeroed sectors, `len`
    /// set, and sector `idx`'s `quote_authority` = [`AUTH`]. Mirrors the
    /// `Slab` framing the on-chain account uses.
    fn market_buf(auth_idx: usize) -> Vec<u8> {
        let mut data = vec![0u8; ITEMS_OFF + SECTORS * VAULT_SIZE];
        data[LEN_OFF..LEN_OFF + 4].copy_from_slice(&(SECTORS as u32).to_le_bytes());
        let auth_off = ITEMS_OFF + auth_idx * VAULT_SIZE + VAULT_QUOTE_AUTHORITY_OFF;
        data[auth_off..auth_off + 32].copy_from_slice(&AUTH);
        data
    }

    fn ref_price_bytes(data: &[u8], idx: usize) -> (u64, u32, u32) {
        let rp = ITEMS_OFF + idx * VAULT_SIZE + VAULT_REFERENCE_PRICE_OFF;
        (
            read_u64(data, rp + RP_STAMP_OFF),
            read_u32(data, rp + RP_PRICE_OFF),
            read_u32(data, rp + RP_QUOTE_SLOT_OFF),
        )
    }

    #[test]
    fn error_codes_match_dropset() {
        // anchor-lang-v2 `#[error_code]` maps a fieldless variant to
        // `Custom(index + 6000)`; pin the kernel's domain codes to that so
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
    fn happy_path_stamps_and_bumps_nonce() {
        let mut data = market_buf(2);
        write_u64(&mut data, NONCE_OFF, 41);
        stamp_reference_price(&mut data, 2, 0xDEAD_BEEF, 7, &AUTH).expect("authorized stamp");
        // Nonce advanced by one.
        assert_eq!(read_u64(&data, NONCE_OFF), 42);
        // Stamp carries the OLD nonce OR'd with the flush bit.
        let (stamp, price, slot) = ref_price_bytes(&data, 2);
        assert_eq!(stamp, 41 | FLUSH_BIT);
        assert_eq!(price, 0xDEAD_BEEF);
        assert_eq!(slot, 7);
    }

    #[test]
    fn flush_bit_is_set_even_from_zero_nonce() {
        let mut data = market_buf(0);
        stamp_reference_price(&mut data, 0, 1, 1, &AUTH).unwrap();
        let (stamp, _, _) = ref_price_bytes(&data, 0);
        assert_eq!(stamp, FLUSH_BIT);
        assert_eq!(read_u64(&data, NONCE_OFF), 1);
    }

    #[test]
    fn nonce_wraps_at_u64_max() {
        let mut data = market_buf(0);
        write_u64(&mut data, NONCE_OFF, u64::MAX);
        stamp_reference_price(&mut data, 0, 1, 1, &AUTH).unwrap();
        // Old nonce (all ones) already has the flush bit set.
        let (stamp, _, _) = ref_price_bytes(&data, 0);
        assert_eq!(stamp, u64::MAX);
        assert_eq!(read_u64(&data, NONCE_OFF), 0);
    }

    #[test]
    fn wrong_authority_rejected_without_side_effects() {
        let mut data = market_buf(1);
        write_u64(&mut data, NONCE_OFF, 5);
        let before = data.clone();
        assert_eq!(
            stamp_reference_price(&mut data, 1, 9, 9, &OTHER),
            Err(err::UNAUTHORIZED)
        );
        // Nonce not bumped, price not written.
        assert_eq!(data, before);
    }

    #[test]
    fn out_of_range_index_rejected() {
        let mut data = market_buf(0);
        let before = data.clone();
        // `SECTORS` is one past the last live sector.
        assert_eq!(
            stamp_reference_price(&mut data, SECTORS as u32, 1, 1, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        // The null-sector sentinel is the worst case.
        assert_eq!(
            stamp_reference_price(&mut data, u32::MAX, 1, 1, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        assert_eq!(data, before);
    }

    #[test]
    fn index_within_len_but_past_capacity_rejected() {
        // `len` claims more sectors than the buffer physically holds (the
        // post-external-resize edge `Slab::effective_len` guards). The
        // capacity leg must still reject, matching `min(len, capacity)`.
        let mut data = market_buf(0);
        write_u32(&mut data, LEN_OFF, (SECTORS as u32) + 2);
        assert_eq!(
            stamp_reference_price(&mut data, SECTORS as u32, 1, 1, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
    }

    #[test]
    fn kernel_reads_authority_where_typed_slab_writes_it() {
        // Cross-check the kernel's raw-byte offsets against the real
        // `Slab<MarketHeader, Vault>`: write `quote_authority` through the
        // typed API, then confirm the kernel finds it at the same place
        // when handed the account's raw data bytes. Guards against a
        // coordinate mismatch between the synthetic buffers above and the
        // on-chain layout.
        use super::super::test_support::{load_market, setup};
        let buf = setup();
        {
            let mut market = load_market(&buf);
            market.as_mut_slice()[1].quote_authority = AUTH.into();
        }
        let mut data = buf.read_data().to_vec();
        stamp_reference_price(&mut data, 1, 0xFEED, 9, &AUTH)
            .expect("kernel must find quote_authority at the typed-slab offset");
        // Write the kernel's mutation back and confirm the typed API reads
        // the stamp off the same sector — write offsets agree too.
        buf.write_data(&data);
        let market = load_market(&buf);
        let rp = &market.as_slice()[1].reference_price;
        assert_eq!(rp.price.as_u32(), 0xFEED);
        assert_eq!(rp.quote_slot.get(), 9);
    }

    #[test]
    fn stamps_do_not_bleed_into_neighbors() {
        let mut data = market_buf(1);
        stamp_reference_price(&mut data, 1, 0xABCD, 3, &AUTH).unwrap();
        // Neighboring sectors' reference prices stay zeroed.
        assert_eq!(ref_price_bytes(&data, 0), (0, 0, 0));
        assert_eq!(ref_price_bytes(&data, 2), (0, 0, 0));
    }
}
