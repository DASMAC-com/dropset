//! Quote ladders → on-chain [`LiquidityProfile`], and the passive-ladder
//! shape rules.
//!
//! A maker quotes *relatively*: a symmetric ladder of per-level ppm offsets
//! and bps sizes around the reference price. Sizes are fractions of the
//! inventory leg, which the program auto-rescales to current inventory on each
//! flush — so the profile is a function of the *strategy*, not the live
//! balance.
//!
//! Both surfaces that deploy a passive book share these rules: the TUI's
//! set-and-forget action builds through [`PassiveLadder::levels`], the maker
//! bot's default passive ladder is pinned equal to its default output by a
//! test, and both check with [`passive_shape_warnings`].

use bytemuck::Zeroable;

use crate::clock::{SlotSpan, WallSpan};
use crate::layout::{LiquidityProfile, BPS, N_LEVELS};

/// One rung of the quote ladder: a ppm offset from the reference price, a
/// fraction of the inventory leg in bps, a flip spread, and a per-level
/// expiry in each of the two domains the engine gates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LadderLevel {
    /// Offset from `reference_price`, in ppm (bids subtract, asks add).
    pub offset_ppm: u32,
    /// Flip spread, in ppm off this level's own price: once filled, the
    /// proceeds rest on the opposite side that far back toward the reference.
    /// `0` does not flip — the active ladder's setting, since an active bot
    /// re-stamps faster than a flip would earn.
    pub flip_spread_ppm: u32,
    /// Fraction of the inventory leg, in bps (10000 = 100%).
    pub size_bps: u16,
    /// Seconds after the quote's `quote_unix` wall-clock datum at which
    /// this level expires.
    pub expiry_offset_secs: WallSpan,
    /// Slots after the quote's `quote_slot` datum at which this level
    /// expires — the second, independent bound.
    /// [`SlotSpan::UNBOUNDED`] leaves a level bounded only in wall time.
    pub expiry_offset_slots: SlotSpan,
}

/// Build a symmetric [`LiquidityProfile`] from `ladder` — the same offsets,
/// sizes, and expiries on both the bid and ask sides. Levels past
/// [`N_LEVELS`] are dropped.
pub fn build_profile(ladder: &[LadderLevel]) -> LiquidityProfile {
    let mut profile = LiquidityProfile::zeroed();
    for (i, lvl) in ladder.iter().take(N_LEVELS).enumerate() {
        for side in [&mut profile.bids, &mut profile.asks] {
            side[i].price_offset = lvl.offset_ppm.into();
            side[i].flip_spread_ppm = lvl.flip_spread_ppm.into();
            side[i].size_bps = lvl.size_bps.into();
            side[i].expiry_offset_secs = lvl.expiry_offset_secs.get().into();
            side[i].expiry_offset_slots = lvl.expiry_offset_slots.get().into();
        }
    }
    profile
}

/// Shape guidance for a passive ladder, as one human-readable line per
/// problem — empty when the ladder is sound. Guidance, not a gate: the
/// protocol checks none of this, and staying a good LP is the maker's call.
///
/// An empty ladder covers no range. Otherwise the ladder is checked for the
/// three ways its shape goes wrong:
///
/// * **Grow-outward sizes.** A flipped level keeps its size and only changes
///   direction, so a rung larger than the one inside it ends up next to the
///   market after a move — the shape inverts. A passive ladder should be a
///   bell (or uniform, the no-view option).
/// * **No flip spread.** A passive level that does not flip sits filled and
///   dark until the next re-center, which may never come.
/// * **Flip spread past the spacing.** After a fill the market is
///   `spacing + flip_spread` wide; a flip spread past the gap to the next rung
///   in quotes the flipped side through that rung (and the innermost rung's
///   past the reference).
pub fn passive_shape_warnings(ladder: &[LadderLevel]) -> Vec<String> {
    if ladder.is_empty() {
        return vec!["the passive ladder is empty: every tick would re-center".to_string()];
    }
    let mut warnings = Vec::new();
    let mut inner_offset = 0;
    for (i, lvl) in ladder.iter().enumerate() {
        let rung = i + 1;
        if i > 0 && lvl.size_bps > ladder[i - 1].size_bps {
            warnings.push(format!(
                "L{rung} ({} bps) is larger than L{i} ({} bps): a grow-outward passive \
                 ladder inverts after every move — use a bell or uniform shape",
                lvl.size_bps,
                ladder[i - 1].size_bps
            ));
        }
        let spacing = lvl.offset_ppm.saturating_sub(inner_offset);
        if lvl.flip_spread_ppm == 0 {
            warnings.push(format!(
                "L{rung} has no flip spread: once filled it stays dark until a re-center"
            ));
        } else if lvl.flip_spread_ppm > spacing {
            let crossed = if i == 0 {
                "the reference"
            } else {
                "the next level in"
            };
            warnings.push(format!(
                "L{rung} flip spread {} ppm exceeds its {spacing} ppm spacing: the flipped \
                 side would quote through {crossed}",
                lvl.flip_spread_ppm
            ));
        }
        inner_offset = lvl.offset_ppm;
    }
    warnings
}

/// The passive ladder's default level spacing: 5 bps between adjacent rungs,
/// the first rung 5 bps off the reference.
pub const PASSIVE_SPACING_PPM: u32 = 500;

/// The passive ladder's default flip spread: 2 bps, inside the 5 bps spacing,
/// so a filled level's flipped side never crosses the next rung in. That is
/// the maker's whole earn per round trip — the taker fee is protocol revenue
/// — so 2 bps is the economics of a 1 bp concentrated-liquidity pool.
pub const PASSIVE_FLIP_SPREAD_PPM: u32 = 200;
const _: () = assert!(PASSIVE_FLIP_SPREAD_PPM <= PASSIVE_SPACING_PPM);

/// The hand-shaped bell over the full [`N_LEVELS`] rungs, innermost first:
/// largest at the center and shrinking outward. `Σ = BPS`, so the full-width
/// bell is these sizes exactly; a narrower one renormalizes a prefix.
pub const BELL_SIZES_BPS: [u16; N_LEVELS] = [2_000, 1_800, 1_500, 1_300, 1_100, 900, 800, 600];

/// How a passive ladder spreads the inventory leg across its rungs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PassiveShape {
    /// Largest at the center, shrinking outward — the shape that survives a
    /// flip, because the size near the market stays largest whichever way the
    /// levels have turned.
    #[default]
    Bell,
    /// Equal sizes — the no-view option.
    Uniform,
}

/// The parameters of a symmetric passive ladder; [`Self::levels`] expands
/// them into rungs. The default is the maker bot's shipped passive ladder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassiveLadder {
    /// Distance between adjacent rungs, and from the reference to the first.
    pub spacing_ppm: u32,
    /// Rungs per side, clamped to `1..=N_LEVELS`.
    pub levels: usize,
    /// How the leg spreads across the rungs.
    pub shape: PassiveShape,
    /// Every rung's flip spread; [`passive_shape_warnings`] flags one past the
    /// spacing.
    pub flip_spread_ppm: u32,
    /// Every rung's wall-clock life. A passive book is meant to be walked away
    /// from, so the default is [`WallSpan::UNBOUNDED`]: nothing but a
    /// re-center or a kill switch ends it. The slot domain is always unbounded.
    pub expiry_secs: WallSpan,
}

impl Default for PassiveLadder {
    fn default() -> Self {
        Self {
            spacing_ppm: PASSIVE_SPACING_PPM,
            levels: N_LEVELS,
            shape: PassiveShape::Bell,
            flip_spread_ppm: PASSIVE_FLIP_SPREAD_PPM,
            expiry_secs: WallSpan::UNBOUNDED,
        }
    }
}

impl PassiveLadder {
    /// The rungs, innermost first, committing the full leg (`Σ size_bps =
    /// BPS`). Sizes are floored and the remainder goes to the innermost rung,
    /// so flooring never makes a rung larger than the one inside it.
    pub fn levels(&self) -> Vec<LadderLevel> {
        let n = self.levels.clamp(1, N_LEVELS);
        let weights: Vec<u64> = match self.shape {
            PassiveShape::Bell => BELL_SIZES_BPS[..n].iter().map(|&w| w as u64).collect(),
            PassiveShape::Uniform => vec![1; n],
        };
        let total: u64 = weights.iter().sum();
        let mut sizes: Vec<u64> = weights.iter().map(|w| w * BPS / total).collect();
        sizes[0] += BPS - sizes.iter().sum::<u64>();
        sizes
            .into_iter()
            .zip(1u32..)
            .map(|(size, rung)| LadderLevel {
                offset_ppm: rung.saturating_mul(self.spacing_ppm),
                flip_spread_ppm: self.flip_spread_ppm,
                size_bps: size as u16,
                expiry_offset_secs: self.expiry_secs,
                expiry_offset_slots: SlotSpan::UNBOUNDED,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side_sum(levels: &[LadderLevel]) -> u64 {
        levels.iter().map(|l| l.size_bps as u64).sum()
    }

    #[test]
    fn the_default_ladder_is_the_full_bell_at_the_standard_spacing() {
        let levels = PassiveLadder::default().levels();
        assert_eq!(levels.len(), N_LEVELS);
        for (i, lvl) in levels.iter().enumerate() {
            assert_eq!(lvl.offset_ppm, (i as u32 + 1) * PASSIVE_SPACING_PPM);
            assert_eq!(lvl.size_bps, BELL_SIZES_BPS[i]);
            assert_eq!(lvl.flip_spread_ppm, PASSIVE_FLIP_SPREAD_PPM);
            assert_eq!(lvl.expiry_offset_secs, WallSpan::UNBOUNDED);
            assert_eq!(lvl.expiry_offset_slots, SlotSpan::UNBOUNDED);
        }
        assert!(passive_shape_warnings(&levels).is_empty());
    }

    /// Every shape at every width commits the full leg, never grows outward,
    /// and — at the default flip spread — raises no warning.
    #[test]
    fn every_width_and_shape_commits_the_leg_without_growing_outward() {
        for shape in [PassiveShape::Bell, PassiveShape::Uniform] {
            for n in 1..=N_LEVELS {
                let ladder = PassiveLadder {
                    levels: n,
                    shape,
                    ..PassiveLadder::default()
                };
                let levels = ladder.levels();
                assert_eq!(levels.len(), n);
                assert_eq!(side_sum(&levels), BPS, "{shape:?} x{n}");
                let warnings = passive_shape_warnings(&levels);
                assert!(warnings.is_empty(), "{shape:?} x{n}: {warnings:?}");
            }
        }
    }

    #[test]
    fn the_level_count_is_clamped_to_the_profile() {
        let wide = PassiveLadder {
            levels: N_LEVELS + 3,
            ..PassiveLadder::default()
        };
        assert_eq!(wide.levels().len(), N_LEVELS);
        let none = PassiveLadder {
            levels: 0,
            ..PassiveLadder::default()
        };
        assert_eq!(none.levels().len(), 1);
    }

    #[test]
    fn a_flip_spread_past_the_spacing_is_warned_on_every_rung() {
        let ladder = PassiveLadder {
            levels: 3,
            flip_spread_ppm: PASSIVE_SPACING_PPM + 1,
            ..PassiveLadder::default()
        };
        let warnings = passive_shape_warnings(&ladder.levels());
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[0].ends_with("through the reference"));
        assert!(warnings[1].ends_with("through the next level in"));
    }

    #[test]
    fn carries_each_levels_flip_spread_onto_both_sides() {
        let levels = PassiveLadder::default().levels();
        let p = build_profile(&levels);
        for (i, lvl) in levels.iter().enumerate() {
            assert_eq!(p.bids[i].flip_spread_ppm.get(), lvl.flip_spread_ppm);
            assert_eq!(p.asks[i].flip_spread_ppm.get(), lvl.flip_spread_ppm);
            assert_eq!(p.bids[i].price_offset.get(), lvl.offset_ppm);
            assert_eq!(p.asks[i].size_bps.get(), lvl.size_bps);
        }
        assert!(p.validate_size_sums().is_ok());
    }
}
