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
/// Returns `None` when `size_bps > BPS`. Nothing bounds that at *write*
/// time: `set_liquidity_profile` stores the ladder raw (and its ASM fast
/// path validates nothing at all), and `size_bps` is a `u16`, so a
/// *stored* level above `BPS` is reachable from an ordinary,
/// correctly-signed profile write, not just from corrupt account bytes.
/// The per-side `Σ size_bps ≤ BPS` invariant is enforced *solely* at match
/// time — which is also what keeps this `None` arm itself unreachable, as
/// the next paragraph describes. Where it holds the product is at
/// most `leg_atoms * BPS`, which divided by `BPS` is
/// `<= leg_atoms <= u64::MAX`, so the cast is lossless.
///
/// Both callers guard this at the *side* granularity before calling: a side
/// whose `Σ size_bps > BPS` is thrown out of matching whole (the engine
/// zeroes its `remaining`; the simulator skips that vault's side — see
/// `matching::flush_side_sum_exceeds_bps`), so on any side that is still
/// materialized every level is `<= Σ <= BPS` and `None` is unreachable.
/// Callers therefore treat `None` as an unreachable `0` fallback rather
/// than aborting the take — skipping an oversized side is strictly safer
/// than letting one corrupt vault reject every taker.
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

/// Taker fee on a single leg: `output_leg_atoms × taker_fee_ppm / PPM`
/// (u128, truncating). `output_leg_atoms` is the *output* leg the fee is
/// charged on — base atoms on a Buy, quote atoms on a Sell — and
/// `taker_fee_ppm` is the market header's `taker_fee`. Returns the raw
/// u128 product; callers clamp to `u64` themselves (the engine per leg,
/// the simulator after summing every leg's fee), so the byte-identical
/// truncation lives here while each side keeps its own accumulation.
#[inline]
pub fn taker_fee_atoms(output_leg_atoms: u64, taker_fee_ppm: u128) -> u128 {
    (output_leg_atoms as u128 * taker_fee_ppm) / PPM as u128
}

/// Caller-declared platform (integrator) fee on the taker's output:
/// `net_output_atoms × platform_fee_bps / BPS` (u128, truncating — the
/// taker keeps the dust, matching [`taker_fee_atoms`]).
///
/// `net_output_atoms` is the output leg **after** the taker fee, because
/// the two fees compose in program order: fill → taker fee → platform
/// fee. Feeding it the gross leg would over-charge the integrator by
/// `taker_fee × platform_fee`, and — since the engine skims the platform
/// fee from what is left in the treasury after the taker's own transfer —
/// could over-draw the output leg outright.
///
/// Denominated in **bps**, not ppm like the taker fee: the platform fee is
/// the integrator-facing knob, and every surface that carries it across the
/// boundary — the `swap` instruction's `platform_fee_bps` argument, the
/// frontend's `NEXT_PUBLIC_PLATFORM_FEE_BPS`, DFlow's `platformFeeBps` on
/// the route the eCLOB path has to price against — already speaks bps.
/// Converting at each edge would be three chances to drop a factor of 100.
///
/// The caller clamps to `u64` (the engine once per swap, the simulator
/// after summing every leg), so the truncation lives here while each side
/// keeps its own accumulation — same split as [`taker_fee_atoms`].
#[inline]
pub fn platform_fee_atoms(net_output_atoms: u64, platform_fee_bps: u16) -> u128 {
    (net_output_atoms as u128 * platform_fee_bps as u128) / BPS as u128
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

    #[test]
    fn taker_fee_truncates() {
        // 30 ppm on 1_000_000 atoms = 30.
        assert_eq!(taker_fee_atoms(1_000_000, 30), 30);
        // Truncates toward zero: 1 ppm on 1_999_999 = 1.999999 -> 1.
        assert_eq!(taker_fee_atoms(1_999_999, 1), 1);
        // Zero fee and zero leg both yield zero.
        assert_eq!(taker_fee_atoms(1_000_000, 0), 0);
        assert_eq!(taker_fee_atoms(0, 30), 0);
        // No u64 overflow in the product (u128 intermediate).
        assert_eq!(taker_fee_atoms(u64::MAX, PPM as u128), u64::MAX as u128);
    }

    #[test]
    fn platform_fee_truncates() {
        // 30 bps on 1_000_000 atoms = 3_000 — the bps denominator, not ppm.
        // Pinning this against the taker-fee case above is the point: the
        // same numeric rate means a 100x larger fee here, so a denominator
        // mix-up shows up as a failing assert rather than a silent
        // 100x-under-charge on the integrator's cut.
        assert_eq!(platform_fee_atoms(1_000_000, 30), 3_000);
        assert_eq!(taker_fee_atoms(1_000_000, 30), 30);
        // Truncates toward zero (the taker keeps the dust): 1 bps on 19_999
        // = 1.9999 -> 1.
        assert_eq!(platform_fee_atoms(19_999, 1), 1);
        // Below one atom of fee the integrator earns nothing at all.
        assert_eq!(platform_fee_atoms(9_999, 1), 0);
        // Zero rate and zero leg both yield zero — the no-integrator path.
        assert_eq!(platform_fee_atoms(1_000_000, 0), 0);
        assert_eq!(platform_fee_atoms(0, 30), 0);
        // A full-BPS rate takes the whole net leg and no more, with the
        // u128 intermediate absorbing the widest product.
        assert_eq!(platform_fee_atoms(u64::MAX, BPS as u16), u64::MAX as u128);
    }
}
