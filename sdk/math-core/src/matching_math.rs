//! Pure, consensus-critical matcher math — the pieces the on-chain engine
//! (`programs/dropset/src/instructions/swap.rs`) and the off-chain
//! simulator (`dropset_interface::matching`) must compute byte-identically
//! or a router quoting off the simulator produces fills the live engine
//! won't honor.
//!
//! Only the *pure* arithmetic lives here: flush-level pricing, the
//! size-bps fill cap, and the price-time sort key. The iteration / IO
//! around them — walking the on-chain slab vs. reconstructing a book —
//! stays distinct in each caller. This module is `core`-only (it pulls no
//! `std`), so the on-chain program depends on it without the off-chain
//! book-reconstruction surface in `dropset-interface`.

use crate::price::Price;
use crate::{BPS, PPM};

/// Materialize an absolute-price `Price` from a reference price and a ppm
/// offset. For asks: `ref × (PPM + offset) / PPM`. For bids:
/// `ref × max(PPM − offset, 0) / PPM` (saturating; a bid with offset ≥
/// PPM produces [`Price::ZERO`], which the limit-price filter then
/// excludes). The sentinels pass through unchanged.
#[inline]
pub fn flush_level_price(reference: Price, offset_ppm: u32, is_ask: bool) -> Price {
    if reference.is_zero() || reference.is_infinity() {
        return reference;
    }
    let sig = reference.significand() as u128;
    let exp = reference.biased_exponent() as i16;
    let factor: u128 = if is_ask {
        PPM as u128 + offset_ppm as u128
    } else {
        (PPM as u128).saturating_sub(offset_ppm as u128)
    };
    if factor == 0 {
        return Price::ZERO;
    }
    let scaled = (sig * factor) / (PPM as u128);
    Price::from_scaled(scaled as u64, exp).unwrap_or(Price::ZERO)
}

/// A level's materialized size in atoms: `size_bps` of the matching
/// inventory leg (`base_atoms` for asks, `quote_atoms` for bids).
///
/// Returns `None` when `size_bps > BPS`. `set_liquidity_profile` bounds
/// the per-side Σ `size_bps` to `BPS`, and each `size_bps` is a
/// non-negative `u16`, so every *individual* level is `<= BPS` for any
/// profile written through that path — the `None` case is implied by the
/// sum check and only fires on account bytes the program never wrote
/// (corruption, or a future profile-writing path that skips the sum
/// check). With the invariant held the product is at most `leg_atoms *
/// BPS`, which divided by `BPS` is `<= leg_atoms <= u64::MAX`, so the cast
/// is lossless. Both callers refuse the take on `None` rather than
/// silently clamping (which would mask the bug by shrinking the level):
/// the on-chain caller maps it to a hard error that aborts the whole
/// `swap`, and the off-chain simulator yields an empty quote (it rejects
/// the whole take up front — see `matching::vault_has_oversize_flush_level`
/// — so a router never quotes a fill the engine won't honor).
#[inline]
pub fn level_fill_atoms(size_bps: u16, leg_atoms: u64) -> Option<u64> {
    if size_bps as u64 > BPS {
        return None;
    }
    Some((leg_atoms as u128 * size_bps as u128 / BPS as u128) as u64)
}

/// Cross-vault matching sort key: asks order by raw [`Price::as_u32`]
/// (cheapest ask fills first), bids by [`Price::bid_key`] (highest bid
/// fills first). Combined with `(nonce, sector, level)` this yields the
/// spec's price-time priority from a single sort.
#[inline]
pub fn sort_key(price: Price, is_ask: bool) -> u32 {
    if is_ask {
        price.as_u32()
    } else {
        price.bid_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flush_ask_and_bid_offsets() {
        // EUR/USD 1.0850, ±500 ppm.
        let reference = Price::encode(10_850_000, 0).unwrap();
        let ask = flush_level_price(reference, 500, true);
        let bid = flush_level_price(reference, 500, false);
        assert_eq!(ask.significand(), 10_855_425);
        assert_eq!(bid.significand(), 10_844_575);
        assert!(ask > reference && bid < reference);
    }

    #[test]
    fn flush_sentinels_pass_through() {
        assert_eq!(flush_level_price(Price::ZERO, 500, true), Price::ZERO);
        assert_eq!(
            flush_level_price(Price::INFINITY, 500, false),
            Price::INFINITY
        );
    }

    #[test]
    fn flush_bid_offset_at_or_above_ppm_is_zero() {
        let reference = Price::encode(50_000_000, 0).unwrap();
        assert_eq!(flush_level_price(reference, PPM as u32, false), Price::ZERO);
    }

    #[test]
    fn fill_cap_bounds() {
        assert_eq!(level_fill_atoms(BPS as u16, 1_000_000), Some(1_000_000));
        assert_eq!(level_fill_atoms(5_000, 1_000_000), Some(500_000));
        assert_eq!(level_fill_atoms(0, 1_000_000), Some(0));
        // size_bps above BPS is rejected.
        assert_eq!(level_fill_atoms(BPS as u16 + 1, 1_000_000), None);
    }

    #[test]
    fn sort_key_sides() {
        let p = Price::encode(10_850_000, 0).unwrap();
        assert_eq!(sort_key(p, true), p.as_u32());
        assert_eq!(sort_key(p, false), p.bid_key());
    }
}
