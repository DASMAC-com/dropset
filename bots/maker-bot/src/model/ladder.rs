//! The quote ladder → on-chain `LiquidityProfile` (§2).
//!
//! The bot quotes *relatively*: a symmetric ladder of per-level ppm offsets
//! and bps sizes around the reference price. Sizes are fractions of the
//! inventory leg, which the program auto-rescales to current inventory on each
//! flush — so the profile is a function of the *strategy*, not the live
//! balance. Inventory drift is handled by skewing the reference price instead
//! (see [`super::skew`]), and only a large imbalance reshapes the ladder.

use crate::config::LadderLevel;
use bytemuck::Zeroable;
use dropset_sdk::layout::{LiquidityProfile, N_LEVELS};
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

/// Serialize a profile to the `[u8; 160]` `set_liquidity_profile` argument.
pub fn to_bytes(profile: &LiquidityProfile) -> [u8; 160] {
    profile_bytes(profile)
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
    fn serializes_to_160_bytes() {
        let p = build_profile(&DEFAULT_LADDER);
        assert_eq!(to_bytes(&p).len(), 160);
    }
}
