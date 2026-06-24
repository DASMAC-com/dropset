//! Inventory skew — shift the *reference price*, not the ladder (§2).
//!
//! The formal Avellaneda–Stoikov reservation-price skew `r = mid − q·γ·σ²·τ`
//! comes out sub-bps at this vault's size because the stable-pair σ is tiny,
//! so the spec overrides it with a linear hand-tuned rule: shift the reference
//! by a fixed bps per $10 of signed inventory deviation, capped. When the
//! vault is base-heavy the reference shifts *down*, cheapening our asks and
//! backing off our bids so takers rebalance us toward neutral.

use crate::config::StrategyConfig;
use crate::model::inventory::Inventory;

/// The reference-price skew for `inventory`, in bps (signed: negative when
/// base-heavy). Linear in the deviation, clamped to `±skew_cap_bps`.
pub fn ref_skew_bps(inventory: &Inventory, strat: &StrategyConfig) -> f64 {
    let deviation = inventory.deviation_usd();
    let raw = -(deviation / 10.0) * strat.skew_bps_per_10usd;
    raw.clamp(-strat.skew_cap_bps, strat.skew_cap_bps)
}

/// Apply a bps skew to a mid price, yielding the reference price to stamp.
pub fn apply_skew(mid: f64, skew_bps: f64) -> f64 {
    mid * (1.0 + skew_bps / 10_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strat() -> StrategyConfig {
        StrategyConfig::default()
    }

    fn inv(dev_usd: f64) -> Inventory {
        // Construct legs with the requested signed deviation: dev = (b−q)/2.
        Inventory {
            base_value_usd: 50.0 + dev_usd,
            quote_value_usd: 50.0 - dev_usd,
        }
    }

    #[test]
    fn neutral_inventory_has_no_skew() {
        assert_eq!(ref_skew_bps(&inv(0.0), &strat()), 0.0);
    }

    #[test]
    fn base_heavy_skews_reference_down() {
        // +$10 deviation → −5 bps (5 bps per $10), reference shifts down.
        let skew = ref_skew_bps(&inv(10.0), &strat());
        assert!((skew - -5.0).abs() < 1e-9);
        assert!(apply_skew(0.73, skew) < 0.73);
    }

    #[test]
    fn quote_heavy_skews_reference_up() {
        let skew = ref_skew_bps(&inv(-10.0), &strat());
        assert!((skew - 5.0).abs() < 1e-9);
        assert!(apply_skew(0.73, skew) > 0.73);
    }

    #[test]
    fn skew_is_capped() {
        // A $100 deviation would be −50 bps uncapped; clamps to −20.
        let skew = ref_skew_bps(&inv(100.0), &strat());
        assert_eq!(skew, -20.0);
    }
}
