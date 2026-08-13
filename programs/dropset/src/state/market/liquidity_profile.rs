//! `write_liquidity_profile` — the solana-free kernel behind the
//! `SetLiquidityProfile` leader path.
//!
//! Sibling of [`super::stamp_reference_price`]: same shared preamble from
//! [`super::quote_write`] (sector bounds, the `quote_authority` compare,
//! the nonce bump and flush arm), a different payload — the full 224-byte
//! [`LiquidityProfile`] blob copied into the target sector, leaving
//! `reference_price.price` and `quote_slot` untouched. The hand-written
//! sBPF `entrypoint.s` mirrors it byte-for-byte, using `sol_memcpy_` for
//! the copy; see the architecture spec's **SetLiquidityProfile**.
//!
//! The kernel validates nothing about the profile's *contents*. The
//! per-side `Σ size_bps ≤ BPS` invariant is enforced authoritatively at
//! match time — [`Vault::materialize_remaining`](super::Vault::materialize_remaining)
//! zeroes an over-cap side out of the book instead of aborting the taker's
//! swap — so the write path neither needs nor duplicates that check.

use core::mem::{offset_of, size_of};

use super::quote_write::{authorize_quote_write, bump_nonce_and_arm_flush};
use super::{LiquidityProfile, Vault, N_LEVELS};

/// The `profile` field's offset within a `Vault` sector.
const VAULT_PROFILE_OFF: usize = offset_of!(Vault, profile);
/// On-wire width of the profile blob — what the instruction carries and
/// what the ASM hands `sol_memcpy_` as its length.
pub const PROFILE_SIZE: usize = size_of::<LiquidityProfile>();

// Pinned literals, so a `layout.rs` reorder or an `N_LEVELS` change breaks
// the build here rather than leaving the assembly copying to the wrong
// offset or the wrong width. `PROFILE_SIZE` is cross-checked against the
// per-level derivation `layout.rs` asserts (`2 * N_LEVELS * 14`).
const _: () = assert!(VAULT_PROFILE_OFF == 148);
const _: () = assert!(PROFILE_SIZE == 224);
const _: () = assert!(PROFILE_SIZE == 2 * N_LEVELS * 14);

/// Write `profile_bytes` onto vault `vault_idx`'s ladder, arm the flush
/// bit, and bump the market nonce.
///
/// `data` is the market account's full data region (discriminator
/// included). `signer_key` is the transaction signer's pubkey; the one
/// domain guard is that it equals the target vault's `quote_authority`.
/// The bytes are stored raw — there is no write-time validation of the
/// ladder (see the module docs on where the size invariant lives, and
/// [`authorize_quote_write`] for the guards both quote-write kernels
/// deliberately omit).
///
/// On any domain failure it returns a [`super::err`] code with `data`
/// unmodified: every check runs before the nonce is bumped, so a rejected
/// call never advances market state.
#[inline]
pub fn write_liquidity_profile(
    data: &mut [u8],
    vault_idx: u32,
    profile_bytes: &[u8; PROFILE_SIZE],
    signer_key: &[u8; 32],
) -> Result<(), u32> {
    let vault_off = authorize_quote_write(data, vault_idx, signer_key)?;
    bump_nonce_and_arm_flush(data, vault_off);

    // Payload: the profile blob verbatim. The ASM twin issues this as one
    // `sol_memcpy_`; both write exactly `PROFILE_SIZE` bytes at
    // `vault + VAULT_PROFILE_OFF`, so neither touches the neighboring
    // `reference_price` (below) or `remaining` (above).
    let profile_off = vault_off + VAULT_PROFILE_OFF;
    data[profile_off..profile_off + PROFILE_SIZE].copy_from_slice(profile_bytes);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::quote_write::{
        err, read_u32, read_u64, test_buf::*, write_u32, write_u64, ITEMS_OFF, LEN_OFF, NONCE_OFF,
        RP_STAMP_OFF, VAULT_REFERENCE_PRICE_OFF, VAULT_SIZE,
    };
    use super::*;
    use crate::state::FLUSH_BIT;
    use anchor_lang_v2::bytemuck::{bytes_of, Zeroable};

    /// A recognizable profile blob: byte `i` is `i + 1`, so any
    /// off-by-`n` in the destination offset shows up as a shifted pattern
    /// rather than a plausible-looking zero.
    fn profile_bytes() -> [u8; PROFILE_SIZE] {
        let mut bytes = [0u8; PROFILE_SIZE];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i + 1) as u8;
        }
        bytes
    }

    /// The wire bytes of a typed profile — the shape the instruction arg
    /// carries, for the cases that need to set named `Level` fields rather
    /// than an arbitrary blob.
    fn wire_bytes(profile: &LiquidityProfile) -> [u8; PROFILE_SIZE] {
        let mut bytes = [0u8; PROFILE_SIZE];
        bytes.copy_from_slice(bytes_of(profile));
        bytes
    }

    fn stored_profile(data: &[u8], idx: usize) -> &[u8] {
        let off = ITEMS_OFF + idx * VAULT_SIZE + VAULT_PROFILE_OFF;
        &data[off..off + PROFILE_SIZE]
    }

    fn ref_price_bytes(data: &[u8], idx: usize) -> (u64, u32, u32) {
        let rp = ITEMS_OFF + idx * VAULT_SIZE + VAULT_REFERENCE_PRICE_OFF;
        (
            read_u64(data, rp + RP_STAMP_OFF),
            read_u32(data, rp + 8),
            read_u32(data, rp + 12),
        )
    }

    #[test]
    fn happy_path_writes_profile_and_arms_flush() {
        let mut data = market_buf(2);
        write_u64(&mut data, NONCE_OFF, 41);
        // Pre-existing price / slot the write must preserve.
        let rp = ITEMS_OFF + 2 * VAULT_SIZE + VAULT_REFERENCE_PRICE_OFF;
        write_u32(&mut data, rp + 8, 0xDEAD_BEEF);
        write_u32(&mut data, rp + 12, 7);

        let bytes = profile_bytes();
        write_liquidity_profile(&mut data, 2, &bytes, &AUTH).expect("authorized write");

        assert_eq!(stored_profile(&data, 2), bytes, "profile stored verbatim");
        assert_eq!(read_u64(&data, NONCE_OFF), 42, "nonce advanced by one");
        assert_eq!(
            ref_price_bytes(&data, 2),
            (41 | FLUSH_BIT, 0xDEAD_BEEF, 7),
            "flush armed over the OLD nonce; price / quote_slot untouched"
        );
    }

    #[test]
    fn wrong_authority_rejected_without_side_effects() {
        let mut data = market_buf(1);
        write_u64(&mut data, NONCE_OFF, 5);
        let before = data.clone();
        assert_eq!(
            write_liquidity_profile(&mut data, 1, &profile_bytes(), &OTHER),
            Err(err::UNAUTHORIZED)
        );
        // Nonce not bumped, profile not written.
        assert_eq!(data, before);
    }

    #[test]
    fn out_of_range_index_rejected() {
        let mut data = market_buf(0);
        let before = data.clone();
        // `SECTORS` is one past the last live sector.
        assert_eq!(
            write_liquidity_profile(&mut data, SECTORS, &profile_bytes(), &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        // The null-sector sentinel is the worst case.
        assert_eq!(
            write_liquidity_profile(&mut data, u32::MAX, &profile_bytes(), &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        assert_eq!(data, before);
    }

    #[test]
    fn index_within_len_but_past_capacity_rejected() {
        // `len` claims more sectors than the buffer physically holds (the
        // post-external-resize edge `Slab::effective_len` guards). The
        // capacity leg must still reject, matching `min(len, capacity)` —
        // and it must reject *before* a 224-byte copy runs off the end.
        let mut data = market_buf(0);
        write_u32(&mut data, LEN_OFF, SECTORS + 2);
        assert_eq!(
            write_liquidity_profile(&mut data, SECTORS, &profile_bytes(), &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
    }

    #[test]
    fn over_bps_profile_is_stored_not_rejected() {
        // No write-time `Σ size_bps ≤ BPS` gate: an over-cap ladder is
        // accepted and stored verbatim, and `materialize_remaining` zeroes
        // the offending side out of matching at flush time instead.
        let mut data = market_buf(0);
        let mut over = LiquidityProfile::zeroed();
        over.bids[0].size_bps = 6_000u16.into();
        over.bids[1].size_bps = 5_000u16.into(); // 11_000 > BPS
        let bytes = wire_bytes(&over);

        write_liquidity_profile(&mut data, 0, &bytes, &AUTH)
            .expect("an over-cap ladder is the leader's own problem, not a write-time reject");
        assert_eq!(stored_profile(&data, 0), bytes);
    }

    #[test]
    fn copy_does_not_bleed_past_the_profile_field() {
        // The 224-byte copy must land exactly on `Vault.profile`: the
        // sector's own `reference_price` (below it) keeps its price / slot,
        // its `remaining` (above it) stays zeroed, and the neighboring
        // sectors are untouched.
        let mut data = market_buf(1);
        let rp = ITEMS_OFF + VAULT_SIZE + VAULT_REFERENCE_PRICE_OFF;
        write_u32(&mut data, rp + 8, 0xFEED);
        write_u32(&mut data, rp + 12, 3);

        write_liquidity_profile(&mut data, 1, &profile_bytes(), &AUTH).unwrap();

        // `remaining` occupies the rest of the sector after `profile`.
        let remaining_off = ITEMS_OFF + VAULT_SIZE + VAULT_PROFILE_OFF + PROFILE_SIZE;
        let sector_end = ITEMS_OFF + 2 * VAULT_SIZE;
        assert!(
            data[remaining_off..sector_end].iter().all(|&b| b == 0),
            "`remaining` must stay zeroed — the flush, not the write, fills it"
        );
        assert_eq!(ref_price_bytes(&data, 1).1, 0xFEED, "price preserved");
        assert_eq!(ref_price_bytes(&data, 1).2, 3, "quote_slot preserved");
        // Neighboring sectors' profiles stay zeroed.
        assert!(stored_profile(&data, 0).iter().all(|&b| b == 0));
        assert!(stored_profile(&data, 2).iter().all(|&b| b == 0));
    }

    #[test]
    fn kernel_writes_profile_where_typed_slab_reads_it() {
        // Cross-check the raw-byte offset against the real
        // `Slab<MarketHeader, Vault>`: write through the kernel, then read
        // the ladder back through the typed API. Guards against a
        // coordinate mismatch between the synthetic buffers above and the
        // on-chain layout.
        use super::super::test_support::{load_market, setup};
        let buf = setup();
        {
            let mut market = load_market(&buf);
            market.as_mut_slice()[1].quote_authority = AUTH.into();
        }
        let mut data = buf.read_data().to_vec();
        let mut profile = LiquidityProfile::zeroed();
        profile.asks[0].price_offset = 5_000u32.into();
        profile.asks[0].size_bps = 10_000u16.into();
        let bytes = wire_bytes(&profile);

        write_liquidity_profile(&mut data, 1, &bytes, &AUTH)
            .expect("kernel must find quote_authority at the typed-slab offset");

        buf.write_data(&data);
        let market = load_market(&buf);
        let v = &market.as_slice()[1];
        assert_eq!(v.profile.asks[0].price_offset.get(), 5_000);
        assert_eq!(v.profile.asks[0].size_bps.get(), 10_000);
        assert!(v.reference_price.stamp.get() & FLUSH_BIT != 0);
    }
}
