//! The quote ladder → on-chain `LiquidityProfile` (§2).
//!
//! The bot quotes *relatively*: a symmetric ladder of per-level ppm offsets
//! and bps sizes around the reference price. Sizes are fractions of the
//! inventory leg, which the program auto-rescales to current inventory on each
//! flush — so the profile is a function of the *strategy*, not the live
//! balance. Inventory drift is handled by skewing the reference price instead
//! (see [`super::skew`]), and only a large imbalance reshapes the ladder.

use crate::config::{LadderLevel, StrategyMode};
use anyhow::Result;
use dropset_sdk::ladder::passive_shape_warnings;
use dropset_sdk::layout::{LiquidityProfile, BPS};
use dropset_sdk::quoting::{profile_bytes, PROFILE_BYTES};

/// Which side of the book a reshape targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Bid,
    Ask,
}

pub use dropset_sdk::ladder::build_profile;

/// Shape guidance for a ladder under `mode` (§2), as one human-readable line
/// per problem — empty when the ladder is sound. The bot logs these at
/// startup and quotes the ladder it was given. Active mode has nothing to
/// say; a passive ladder gets the shared [`passive_shape_warnings`] rules,
/// with `tip` declaring its L1 a tip.
pub fn shape_warnings(ladder: &[LadderLevel], mode: StrategyMode, tip: bool) -> Vec<String> {
    match mode {
        StrategyMode::Active => Vec::new(),
        StrategyMode::Passive => passive_shape_warnings(ladder, tip),
    }
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

/// Serialize a profile to the `[u8; PROFILE_BYTES]` `set_liquidity_profile` argument.
pub fn to_bytes(profile: &LiquidityProfile) -> [u8; PROFILE_BYTES] {
    profile_bytes(profile)
}

/// Serialize a profile for submission, first asserting the on-chain
/// match-time gate (per-side `Σ size_bps ≤ BPS`). A side over the cap is
/// silently skipped by the matcher (a no-fill), so an honest bot treats it as
/// the sizing bug it is and refuses to arm a dark side rather than submit it.
/// Every `set_liquidity_profile` send routes through here.
pub fn checked_bytes(profile: &LiquidityProfile) -> Result<[u8; PROFILE_BYTES]> {
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
    use crate::config::{default_passive_ladder, DEFAULT_LADDER};
    use dropset_sdk::layout::N_LEVELS;

    #[test]
    fn carries_each_levels_flip_spread_onto_both_sides() {
        let passive = default_passive_ladder();
        let p = build_profile(&passive);
        for (i, lvl) in passive.iter().enumerate() {
            assert_eq!(p.bids[i].flip_spread_ppm.get(), lvl.flip_spread_ppm);
            assert_eq!(p.asks[i].flip_spread_ppm.get(), lvl.flip_spread_ppm);
        }
        // The active ladder does not flip.
        let active = build_profile(&DEFAULT_LADDER);
        assert!(active.bids.iter().all(|l| l.flip_spread_ppm.get() == 0));
    }

    #[test]
    fn the_shipped_ladders_raise_no_shape_warnings() {
        assert!(shape_warnings(&default_passive_ladder(), StrategyMode::Passive, true).is_empty());
        // Active is never checked, though its flip-less levels would fail the
        // passive rule.
        assert!(shape_warnings(&DEFAULT_LADDER, StrategyMode::Active, false).is_empty());
        assert!(!shape_warnings(&DEFAULT_LADDER, StrategyMode::Passive, false).is_empty());
    }

    #[test]
    fn a_grow_outward_passive_ladder_is_warned() {
        let ladder: Vec<_> = DEFAULT_LADDER
            .iter()
            .map(|l| LadderLevel {
                flip_spread_ppm: 1_000,
                ..*l
            })
            .collect();
        // DEFAULT_LADDER's sizes shrink outward, so as given it passes...
        let warnings = shape_warnings(&ladder, StrategyMode::Passive, false);
        assert!(warnings.is_empty(), "{warnings:?}");
        // ...and with its sizes reversed onto the same offsets it grows
        // outward at every step.
        let mut grown = ladder.clone();
        grown.reverse();
        for (lvl, src) in grown.iter_mut().zip(&ladder) {
            lvl.offset_ppm = src.offset_ppm;
        }
        let warnings = shape_warnings(&grown, StrategyMode::Passive, false);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("grow-outward")));
        // Uniform sizes are the no-view option, not a grow-outward shape.
        let uniform: Vec<_> = ladder
            .iter()
            .map(|l| LadderLevel {
                size_bps: 2_500,
                ..*l
            })
            .collect();
        assert!(shape_warnings(&uniform, StrategyMode::Passive, false).is_empty());
    }

    #[test]
    fn an_empty_passive_ladder_is_warned() {
        let warnings = shape_warnings(&[], StrategyMode::Passive, false);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("empty"));
        assert!(shape_warnings(&[], StrategyMode::Active, false).is_empty());
    }

    #[test]
    fn a_passive_flip_spread_must_be_set_and_inside_the_spacing() {
        let mut ladder = default_passive_ladder();
        // L1's spacing is its distance to the reference.
        ladder[0].flip_spread_ppm = ladder[0].offset_ppm + 1;
        ladder[1].flip_spread_ppm = 0;
        let warnings = shape_warnings(&ladder, StrategyMode::Passive, true);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings[0].starts_with("L1 flip spread"));
        assert!(warnings[0].ends_with("through the reference"));
        assert!(warnings[1].starts_with("L2 has no flip spread"));
        // Exactly the spacing is allowed.
        ladder[0].flip_spread_ppm = ladder[0].offset_ppm;
        ladder[1].flip_spread_ppm = ladder[1].offset_ppm - ladder[0].offset_ppm;
        assert!(shape_warnings(&ladder, StrategyMode::Passive, true).is_empty());
    }

    #[test]
    fn builds_symmetric_profile_from_default_ladder() {
        let p = build_profile(&DEFAULT_LADDER);
        for (i, lvl) in DEFAULT_LADDER.iter().enumerate() {
            assert_eq!(p.bids[i].price_offset.get(), lvl.offset_ppm);
            assert_eq!(p.bids[i].size_bps.get(), lvl.size_bps);
            assert_eq!(p.bids[i].wall_span(), lvl.expiry_offset_secs);
            assert_eq!(p.bids[i].slot_span(), lvl.expiry_offset_slots);
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

    /// The serialized ladder is exactly the width the instruction arg
    /// declares — 2 sides × 8 levels × an 18-byte `Level` (`price_offset`
    /// u32, `flip_spread_ppm` u32, `size_bps` u16, and one u32 expiry
    /// offset per domain).
    #[test]
    fn serializes_to_the_profile_width() {
        let p = build_profile(&DEFAULT_LADDER);
        assert_eq!(to_bytes(&p).len(), PROFILE_BYTES);
        assert_eq!(PROFILE_BYTES, 2 * N_LEVELS * 18);
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
