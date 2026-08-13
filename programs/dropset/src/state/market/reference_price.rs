//! `stamp_reference_price` — the solana-free kernel behind the
//! `SetReferencePrice` leader hot path.
//!
//! One function, operating directly on a market account's data bytes, is
//! the single source of truth for the leader-price stamp: the non-asm
//! Anchor handler calls it, and the hand-written sBPF `entrypoint.s`
//! mirrors it byte-for-byte (see the architecture spec's
//! **SetReferencePrice**). The shared half of that mirroring — sector
//! bounds, the authority compare, the nonce bump and flush arm — lives in
//! [`super::quote_write`] alongside the little-endian byte accessors; this
//! module owns only the price / slot / wall-clock payload.

use core::mem::offset_of;

use super::quote_write::{
    authorize_quote_write, bump_nonce_and_arm_flush, write_u32, VAULT_REFERENCE_PRICE_OFF,
};
use super::ReferencePrice;

// ── Offsets within a `Vault`'s `ReferencePrice` ─────────────────────
const RP_PRICE_OFF: usize = offset_of!(ReferencePrice, price);
const RP_QUOTE_SLOT_OFF: usize = offset_of!(ReferencePrice, quote_slot);
const RP_QUOTE_UNIX_OFF: usize = offset_of!(ReferencePrice, quote_unix);

const _: () = assert!(RP_PRICE_OFF == 8);
const _: () = assert!(RP_QUOTE_SLOT_OFF == 12);
const _: () = assert!(RP_QUOTE_UNIX_OFF == 16);

/// Stamp `(price_bits, quote_slot, quote_unix)` onto vault `vault_idx`'s
/// reference price, arm the flush bit, and bump the market nonce — the
/// entire steady-state leader hot path, expressed as pure byte math.
///
/// `data` is the market account's full data region (discriminator
/// included). `signer_key` is the transaction signer's pubkey; the one
/// domain guard is that it equals the target vault's `quote_authority`
/// (per the architecture spec's **SetReferencePrice**, price / slot /
/// wall-clock values are stored raw — matching skips an invalid price,
/// so no write-time validation is needed). See
/// [`authorize_quote_write`] for the guards both quote-write kernels
/// deliberately omit.
///
/// `quote_unix` is leader-supplied rather than read from the `Clock`
/// sysvar, which keeps this path syscall-free. That is safe by the same
/// argument as `quote_slot` — but note the argument's *shape* differs
/// from a stored absolute expiry: because every level's `expiry_offset`
/// is measured from this datum, a far-future `quote_unix` **extends**
/// the leader's own level lives rather than merely opting out of the
/// protection. It is still self-harm-only, since a leader can already
/// choose arbitrarily long offsets today, so no trust boundary moves;
/// stamping the past kills the leader's own quotes. See the spec's
/// **SetReferencePrice** for the full narrative.
///
/// On any domain failure it returns a [`super::err`] code with `data`
/// unmodified: every check runs before the nonce is bumped, so a rejected
/// call never advances market state.
#[inline]
pub fn stamp_reference_price(
    data: &mut [u8],
    vault_idx: u32,
    price_bits: u32,
    quote_slot: u32,
    quote_unix: u32,
    signer_key: &[u8; 32],
) -> Result<(), u32> {
    let vault_off = authorize_quote_write(data, vault_idx, signer_key)?;
    bump_nonce_and_arm_flush(data, vault_off);

    // Payload: the packed `(price, quote_slot, quote_unix)` as three
    // adjacent u32s.
    let rp_off = vault_off + VAULT_REFERENCE_PRICE_OFF;
    write_u32(data, rp_off + RP_PRICE_OFF, price_bits);
    write_u32(data, rp_off + RP_QUOTE_SLOT_OFF, quote_slot);
    write_u32(data, rp_off + RP_QUOTE_UNIX_OFF, quote_unix);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::quote_write::{
        err, read_u32, read_u64, test_buf::*, write_u32, write_u64, ITEMS_OFF, LEN_OFF, NONCE_OFF,
        RP_STAMP_OFF, VAULT_SIZE,
    };
    use super::*;
    use crate::state::FLUSH_BIT;

    fn ref_price_bytes(data: &[u8], idx: usize) -> (u64, u32, u32, u32) {
        let rp = ITEMS_OFF + idx * VAULT_SIZE + VAULT_REFERENCE_PRICE_OFF;
        (
            read_u64(data, rp + RP_STAMP_OFF),
            read_u32(data, rp + RP_PRICE_OFF),
            read_u32(data, rp + RP_QUOTE_SLOT_OFF),
            read_u32(data, rp + RP_QUOTE_UNIX_OFF),
        )
    }

    #[test]
    fn happy_path_stamps_and_bumps_nonce() {
        let mut data = market_buf(2);
        write_u64(&mut data, NONCE_OFF, 41);
        stamp_reference_price(&mut data, 2, 0xDEAD_BEEF, 7, 1_700_000_000, &AUTH)
            .expect("authorized stamp");
        // Nonce advanced by one.
        assert_eq!(read_u64(&data, NONCE_OFF), 42);
        // Stamp carries the OLD nonce OR'd with the flush bit.
        let (stamp, price, slot, unix) = ref_price_bytes(&data, 2);
        assert_eq!(stamp, 41 | FLUSH_BIT);
        assert_eq!(price, 0xDEAD_BEEF);
        assert_eq!(slot, 7);
        assert_eq!(unix, 1_700_000_000);
    }

    /// The wall-clock datum is stored raw, exactly like the price and the
    /// slot: the kernel neither validates nor clamps it. A zero datum is
    /// the fail-closed case — every level materializes to
    /// `0 + expiry_offset`, far in the past, so the vault is dead until
    /// the leader stamps a real one.
    #[test]
    fn quote_unix_is_stored_raw_including_zero_and_max() {
        let mut data = market_buf(0);
        stamp_reference_price(&mut data, 0, 1, 1, 0, &AUTH).unwrap();
        assert_eq!(ref_price_bytes(&data, 0).3, 0);
        stamp_reference_price(&mut data, 0, 1, 1, u32::MAX, &AUTH).unwrap();
        assert_eq!(ref_price_bytes(&data, 0).3, u32::MAX);
    }

    #[test]
    fn flush_bit_is_set_even_from_zero_nonce() {
        let mut data = market_buf(0);
        stamp_reference_price(&mut data, 0, 1, 1, 1, &AUTH).unwrap();
        let (stamp, ..) = ref_price_bytes(&data, 0);
        assert_eq!(stamp, FLUSH_BIT);
        assert_eq!(read_u64(&data, NONCE_OFF), 1);
    }

    #[test]
    fn nonce_wraps_at_u64_max() {
        let mut data = market_buf(0);
        write_u64(&mut data, NONCE_OFF, u64::MAX);
        stamp_reference_price(&mut data, 0, 1, 1, 1, &AUTH).unwrap();
        // Old nonce (all ones) already has the flush bit set.
        let (stamp, ..) = ref_price_bytes(&data, 0);
        assert_eq!(stamp, u64::MAX);
        assert_eq!(read_u64(&data, NONCE_OFF), 0);
    }

    #[test]
    fn wrong_authority_rejected_without_side_effects() {
        let mut data = market_buf(1);
        write_u64(&mut data, NONCE_OFF, 5);
        let before = data.clone();
        assert_eq!(
            stamp_reference_price(&mut data, 1, 9, 9, 9, &OTHER),
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
            stamp_reference_price(&mut data, SECTORS, 1, 1, 1, &AUTH),
            Err(err::INVALID_SECTOR_INDEX)
        );
        // The null-sector sentinel is the worst case.
        assert_eq!(
            stamp_reference_price(&mut data, u32::MAX, 1, 1, 1, &AUTH),
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
        write_u32(&mut data, LEN_OFF, SECTORS + 2);
        assert_eq!(
            stamp_reference_price(&mut data, SECTORS, 1, 1, 1, &AUTH),
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
        stamp_reference_price(&mut data, 1, 0xFEED, 9, 1_700_000_000, &AUTH)
            .expect("kernel must find quote_authority at the typed-slab offset");
        // Write the kernel's mutation back and confirm the typed API reads
        // the stamp off the same sector — write offsets agree too.
        buf.write_data(&data);
        let market = load_market(&buf);
        let rp = &market.as_slice()[1].reference_price;
        assert_eq!(rp.price.as_u32(), 0xFEED);
        assert_eq!(rp.quote_slot.get(), 9);
        assert_eq!(rp.quote_unix.get(), 1_700_000_000);
    }

    #[test]
    fn stamps_do_not_bleed_into_neighbors() {
        let mut data = market_buf(1);
        stamp_reference_price(&mut data, 1, 0xABCD, 3, 1_700_000_000, &AUTH).unwrap();
        // Neighboring sectors' reference prices stay zeroed.
        assert_eq!(ref_price_bytes(&data, 0), (0, 0, 0, 0));
        assert_eq!(ref_price_bytes(&data, 2), (0, 0, 0, 0));
    }
}
