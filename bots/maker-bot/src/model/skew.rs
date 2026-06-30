//! Inventory skew — shift the *reference price*, not the ladder (§2).
//!
//! The formal Avellaneda–Stoikov reservation-price skew `r = mid − q·γ·σ²·τ`
//! comes out sub-bps at this vault's size because the stable-pair σ is tiny,
//! so the spec overrides it with a linear hand-tuned rule: shift the reference
//! by a fixed bps per unit of signed inventory deviation, capped. When the
//! vault is base-heavy the reference shifts *down*, cheapening our asks and
//! backing off our bids so takers rebalance us toward neutral.
//!
//! The deviation is measured as a **percentage of TVL**, not in absolute
//! dollars. The spec's original "5 bps per $10 of deviation" was calibrated to
//! its $100 reference vault, so on a larger vault any drift instantly saturated
//! the cap and the skew never breathed. Pegging the rate to *fractional*
//! lopsidedness instead makes one calibration correct at every size — the demo
//! runs ~$100 top-of-book per market across seven differently-priced tokens, so
//! the skew must mean the same thing whatever the absolute balances are. At the
//! $100 reference scale the relative rule reproduces the spec's numbers exactly
//! (a $10 deviation is 10% of a $100 TVL → `10 · 0.5 = 5` bps).

use crate::config::StrategyConfig;
use crate::model::inventory::Inventory;

/// The reference-price skew for `inventory`, in bps (signed: negative when
/// base-heavy). Linear in the deviation *as a percentage of TVL*, clamped to
/// `±skew_cap_bps`.
pub fn ref_skew_bps(inventory: &Inventory, strat: &StrategyConfig) -> f64 {
    let deviation_pct = inventory.deviation_pct();
    let raw = -deviation_pct * strat.skew_bps_per_pct_tvl;
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
        // Construct legs with the requested signed deviation at the $100
        // reference TVL: dev = (b−q)/2, so at $100 TVL the deviation in dollars
        // equals the deviation as a percentage of TVL.
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
        // +$10 deviation on the $100 reference vault is 10% of TVL → −5 bps
        // (0.5 bps per 1% of TVL), so the reference shifts down.
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
        // A $100 deviation is 100% of the $100 TVL → −50 bps uncapped; clamps
        // to −20.
        let skew = ref_skew_bps(&inv(100.0), &strat());
        assert_eq!(skew, -20.0);
    }

    #[test]
    fn skew_is_scale_invariant() {
        // The whole point of the relative form: a vault 10,000× larger but
        // equally lopsided gets the *same* skew, where the old per-$10 rule
        // would have pinned it at the cap. 10% base-heavy → −5 bps at any size.
        let small = inv(10.0); // $60 / $40
        let large = Inventory {
            base_value_usd: 600_000.0,
            quote_value_usd: 400_000.0,
        };
        assert!((ref_skew_bps(&small, &strat()) - ref_skew_bps(&large, &strat())).abs() < 1e-9);
        assert!((ref_skew_bps(&large, &strat()) - -5.0).abs() < 1e-9);
    }
}
