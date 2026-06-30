//! Inventory valuation — the bridge from on-chain atoms to the USD-equivalent
//! quantities the skew (§2) and kill-switch (§4) policies reason about.
//!
//! The quote leg is USDC (≈ $1), so its USD value is just its atom count
//! descaled by decimals. The base leg (the FX-stablecoin token) is valued at
//! the current mid (USD per token). "Deviation from neutral" is half the gap
//! between the legs — a $10 swing means each side moved $5 off the midpoint
//! (§2).

/// A vault's two legs, valued in USD.
#[derive(Clone, Copy, Debug)]
pub struct Inventory {
    /// Base leg (the token) value in USD: `base_atoms / 10^decimals · mid`.
    pub base_value_usd: f64,
    /// Quote leg (USDC) value in USD: `quote_atoms / 10^decimals`.
    pub quote_value_usd: f64,
}

impl Inventory {
    /// Value a vault's raw atom balances at `mid` (USDC per base).
    pub fn from_atoms(
        base_atoms: u64,
        quote_atoms: u64,
        base_decimals: u8,
        quote_decimals: u8,
        mid: f64,
    ) -> Self {
        let base_units = base_atoms as f64 / 10f64.powi(base_decimals as i32);
        let quote_units = quote_atoms as f64 / 10f64.powi(quote_decimals as i32);
        Self {
            base_value_usd: base_units * mid,
            quote_value_usd: quote_units,
        }
    }

    /// Total vault value (TVL) in USD.
    pub fn total_usd(&self) -> f64 {
        self.base_value_usd + self.quote_value_usd
    }

    /// Signed deviation from neutral, in USD — positive when base-heavy.
    /// Half the inter-leg gap (§2: a $10 swing is a $5 deviation).
    pub fn deviation_usd(&self) -> f64 {
        (self.base_value_usd - self.quote_value_usd) / 2.0
    }

    /// Signed deviation from neutral as a percentage of TVL — positive when
    /// base-heavy. This is the *scale-free* form the inventory skew (§2) leans
    /// on: the same fractional lopsidedness yields the same skew whether the
    /// vault holds $100 or $1M, so a single calibration is correct across the
    /// demo's seven differently-priced markets. Relates to [`Self::deviation_usd`]
    /// as `deviation_usd / TVL`, and to [`Self::imbalance_pct`] as half of it
    /// (the §2 deviation is the half-gap; imbalance is the full gap).
    pub fn deviation_pct(&self) -> f64 {
        let total = self.total_usd();
        if total <= 0.0 {
            return 0.0;
        }
        self.deviation_usd() / total * 100.0
    }

    /// Imbalance as a percentage of TVL: `|base − quote| / total · 100`. A
    /// 65/35 split reads as 30%, matching the §4 trigger table.
    pub fn imbalance_pct(&self) -> f64 {
        let total = self.total_usd();
        if total <= 0.0 {
            return 0.0;
        }
        (self.base_value_usd - self.quote_value_usd).abs() / total * 100.0
    }

    /// Whether the heavy side is the base leg — the side a §4 reshape grows or
    /// a freeze zeroes.
    pub fn base_heavy(&self) -> bool {
        self.base_value_usd >= self.quote_value_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_vault_has_no_deviation() {
        // 100 EURC at 1.14 = $114; 114 USDC = $114. Balanced at $100-scale.
        let inv = Inventory::from_atoms(100_000_000, 114_000_000, 6, 6, 1.14);
        assert!(inv.deviation_usd().abs() < 1.0);
        assert!(inv.imbalance_pct() < 0.01);
    }

    #[test]
    fn imbalance_matches_the_trigger_table() {
        // $65 base / $35 quote → 30% imbalance (the §4 reshape threshold).
        let inv = Inventory {
            base_value_usd: 65.0,
            quote_value_usd: 35.0,
        };
        assert!((inv.imbalance_pct() - 30.0).abs() < 1e-9);
        assert!(inv.base_heavy());
        // Deviation is half the $30 gap.
        assert!((inv.deviation_usd() - 15.0).abs() < 1e-9);
        // As a fraction of TVL it is half the imbalance: 15% of $100.
        assert!((inv.deviation_pct() - 15.0).abs() < 1e-9);
    }

    #[test]
    fn deviation_pct_is_scale_free() {
        // The same 60/40 lopsidedness reads as the same deviation percentage at
        // $100 and at $1M — that scale-invariance is what lets one skew
        // calibration serve every market size.
        let small = Inventory {
            base_value_usd: 60.0,
            quote_value_usd: 40.0,
        };
        let large = Inventory {
            base_value_usd: 600_000.0,
            quote_value_usd: 400_000.0,
        };
        assert!((small.deviation_pct() - large.deviation_pct()).abs() < 1e-9);
        assert!((small.deviation_pct() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn empty_vault_has_no_deviation_pct() {
        let inv = Inventory {
            base_value_usd: 0.0,
            quote_value_usd: 0.0,
        };
        assert_eq!(inv.deviation_pct(), 0.0);
    }

    #[test]
    fn quote_heavy_vault_reads_as_not_base_heavy() {
        let inv = Inventory {
            base_value_usd: 20.0,
            quote_value_usd: 80.0,
        };
        assert!(!inv.base_heavy());
        assert!((inv.imbalance_pct() - 60.0).abs() < 1e-9);
    }
}
