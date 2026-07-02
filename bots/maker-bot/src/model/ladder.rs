//! The quote ladder → on-chain `LiquidityProfile` (§2).
//!
//! The bot quotes *relatively*: a symmetric ladder of per-level ppm offsets
//! and bps sizes around the reference price. Sizes are fractions of the
//! inventory leg, which the program auto-rescales to current inventory on each
//! flush — so the profile is a function of the *strategy*, not the live
//! balance. Inventory drift is handled by skewing the reference price instead
//! (see [`super::skew`]), and only a large imbalance reshapes the ladder.

use crate::config::LadderLevel;
use anyhow::Result;
use bytemuck::Zeroable;
use dropset_sdk::layout::{LiquidityProfile, BPS, N_LEVELS};
use dropset_sdk::quoting::profile_bytes;

/// Which side of the book a reshape targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

/// Build a symmetric [`LiquidityProfile`] from `ladder` — the same offsets,
/// sizes, and expiries on both the bid and ask sides. Levels past
/// [`N_LEVELS`] are dropped (the config is validated to fit, see tests).
pub fn build_profile(ladder: &[LadderLevel]) -> LiquidityProfile {
    let mut profile = LiquidityProfile::zeroed();
    for (i, lvl) in ladder.iter().take(N_LEVELS).enumerate() {
        for side in [&mut profile.bids, &mut profile.asks] {
            side[i].price_offset = lvl.offset_ppm.into();
            side[i].size_bps = lvl.size_bps.into();
            side[i].expiry_offset = lvl.expiry_offset.into();
        }
    }
    profile
}

/// Zero one side of a profile — the §4 "freeze the heavy side" reshape, where
/// only the rebuild side keeps quoting.
pub fn zero_side(profile: &mut LiquidityProfile, side: Side) {
    let levels = match side {
        Side::Bid => &mut profile.bids,
        Side::Ask => &mut profile.asks,
    };
    for level in levels.iter_mut() {
        level.size_bps = 0u16.into();
    }
}

/// Scale one side's per-level `size_bps` by `scale` — the §4 reshape
/// (imbalance > 30%). The standard ladder already fully commits each leg
/// (`Σ size_bps = 10000`), so "grow the heavy side" (spec §4 row 1) can only
/// be realized *relatively*: shrink the accumulating side here (`scale < 1`)
/// so the untouched heavy (rebuild) side dominates the book and leans into
/// offloading the heavy leg. A milder step than [`zero_side`], which the
/// > 50% freeze uses to drop the accumulating side entirely.
///
/// The per-side `Σ size_bps ≤ BPS` invariant is now enforced at *match* time:
/// a side that sums past BPS is silently thrown out of matching by the engine
/// (a no-fill), not rejected at write time. So this must never leave a side
/// over BPS. Two guards make that hold for any `scale`:
///
/// * **Floor, never round.** Round-nearest can tick an individual level up and
///   drift a `Σ = BPS` side to `BPS + 1`; flooring only ever shrinks, so a
///   `scale ≤ 1` side stays `≤ BPS` by construction.
/// * **Renormalize.** For a hypothetical `scale > 1` grow, if the floored sum
///   still exceeds BPS, scale every level down by `BPS / sum` and floor again
///   — each term lands `≤` its share of BPS, so the new sum is `≤ BPS`.
pub fn scale_side(profile: &mut LiquidityProfile, side: Side, scale: f64) {
    let levels = match side {
        Side::Bid => &mut profile.bids,
        Side::Ask => &mut profile.asks,
    };
    for level in levels.iter_mut() {
        let scaled = (f64::from(level.size_bps.get()) * scale).max(0.0).floor();
        level.size_bps = (scaled.min(f64::from(u16::MAX)) as u16).into();
    }
    let sum: u64 = levels.iter().map(|l| l.size_bps.get() as u64).sum();
    if sum > BPS {
        for level in levels.iter_mut() {
            let capped = level.size_bps.get() as u64 * BPS / sum;
            level.size_bps = (capped as u16).into();
        }
    }
    debug_assert!(
        levels.iter().map(|l| l.size_bps.get() as u64).sum::<u64>() <= BPS,
        "scale_side must never leave a side over BPS"
    );
}

/// Serialize a profile to the `[u8; 160]` `set_liquidity_profile` argument.
pub fn to_bytes(profile: &LiquidityProfile) -> [u8; 160] {
    profile_bytes(profile)
}

/// Serialize a profile for submission, first asserting the on-chain
/// match-time gate (per-side `Σ size_bps ≤ BPS`). A side over the cap is
/// silently skipped by the matcher (a no-fill), so an honest bot treats it as
/// the sizing bug it is and refuses to arm a dark side rather than submit it.
/// Every `set_liquidity_profile` send routes through here.
pub fn checked_bytes(profile: &LiquidityProfile) -> Result<[u8; 160]> {
    if let Err(v) = profile.validate_size_sums() {
        anyhow::bail!(
            "liquidity profile Σ size_bps exceeds {BPS} (bids={}, asks={}) — sizing bug",
            v.bid_sum,
            v.ask_sum
        );
    }
    Ok(to_bytes(profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_LADDER;

    #[test]
    fn builds_symmetric_profile_from_default_ladder() {
        let p = build_profile(&DEFAULT_LADDER);
        for (i, lvl) in DEFAULT_LADDER.iter().enumerate() {
            assert_eq!(p.bids[i].price_offset.get(), lvl.offset_ppm);
            assert_eq!(p.bids[i].size_bps.get(), lvl.size_bps);
            assert_eq!(p.bids[i].expiry_offset.get(), lvl.expiry_offset);
            assert_eq!(p.asks[i].price_offset.get(), lvl.offset_ppm);
            assert_eq!(p.asks[i].size_bps.get(), lvl.size_bps);
        }
        // Unused rungs stay zeroed.
        assert_eq!(p.bids[DEFAULT_LADDER.len()].size_bps.get(), 0);
    }

    #[test]
    fn per_side_size_respects_the_inventory_invariant() {
        let p = build_profile(&DEFAULT_LADDER);
        let bid: u32 = p.bids.iter().map(|l| l.size_bps.get() as u32).sum();
        let ask: u32 = p.asks.iter().map(|l| l.size_bps.get() as u32).sum();
        assert!(bid <= 10_000);
        assert!(ask <= 10_000);
    }

    #[test]
    fn zero_side_clears_only_one_side() {
        let mut p = build_profile(&DEFAULT_LADDER);
        zero_side(&mut p, Side::Ask);
        assert_eq!(p.asks[0].size_bps.get(), 0);
        assert_eq!(p.bids[0].size_bps.get(), DEFAULT_LADDER[0].size_bps);
    }

    #[test]
    fn scale_side_shrinks_only_the_accumulating_side() {
        let mut p = build_profile(&DEFAULT_LADDER);
        scale_side(&mut p, Side::Bid, 0.5);
        // The scaled (accumulating) side halves; the heavy side is untouched
        // and so dominates the book.
        for (i, lvl) in DEFAULT_LADDER.iter().enumerate() {
            assert_eq!(p.bids[i].size_bps.get(), lvl.size_bps / 2);
            assert_eq!(p.asks[i].size_bps.get(), lvl.size_bps);
        }
        let bid: u32 = p.bids.iter().map(|l| l.size_bps.get() as u32).sum();
        let ask: u32 = p.asks.iter().map(|l| l.size_bps.get() as u32).sum();
        assert!(bid < ask);
    }

    #[test]
    fn scale_side_shrinks_the_ask_side_too() {
        // The quote-heavy reshape scales the ask side; the bid side is left at
        // full commit.
        let mut p = build_profile(&DEFAULT_LADDER);
        scale_side(&mut p, Side::Ask, 0.5);
        for (i, lvl) in DEFAULT_LADDER.iter().enumerate() {
            assert_eq!(p.asks[i].size_bps.get(), lvl.size_bps / 2);
            assert_eq!(p.bids[i].size_bps.get(), lvl.size_bps);
        }
    }

    #[test]
    fn serializes_to_160_bytes() {
        let p = build_profile(&DEFAULT_LADDER);
        assert_eq!(to_bytes(&p).len(), 160);
    }

    #[test]
    fn scale_side_never_exceeds_bps_even_when_growing() {
        // A hypothetical `scale > 1` grow can't push a side past BPS — the
        // renormalize caps it — so the match-time gate never silently skips
        // the reshaped side.
        let mut p = build_profile(&DEFAULT_LADDER);
        scale_side(&mut p, Side::Bid, 5.0);
        let bid: u32 = p.bids.iter().map(|l| l.size_bps.get() as u32).sum();
        assert!(bid <= 10_000, "grown side capped at BPS (got {bid})");
        assert!(p.validate_size_sums().is_ok());
    }

    #[test]
    fn scale_side_shrink_below_one_stays_valid() {
        // The real reshape path (`scale ≤ 1`) floors, never rounds up, so the
        // per-side sum can only shrink — even a scale a hair under 1.
        let mut p = build_profile(&DEFAULT_LADDER);
        scale_side(&mut p, Side::Ask, 0.999_9);
        assert!(p.validate_size_sums().is_ok());
        let ask: u32 = p.asks.iter().map(|l| l.size_bps.get() as u32).sum();
        assert!(ask <= 10_000);
    }

    #[test]
    fn checked_bytes_rejects_an_oversized_profile() {
        // No builder path produces `Σ > BPS`; force it directly and confirm
        // the submit guard refuses rather than arm a side the chain would
        // silently skip.
        let mut p = build_profile(&DEFAULT_LADDER);
        p.bids[0].size_bps = (p.bids[0].size_bps.get() + 10_000).into();
        assert!(checked_bytes(&p).is_err());
        // A valid profile serializes cleanly.
        assert!(checked_bytes(&build_profile(&DEFAULT_LADDER)).is_ok());
    }
}
