//! Reference-price composition and the peg sanity bound (§1).
//!
//! For one market, `compose` cascades the tiered feeds primary-first —
//! CoinGecko → CoinMarketCap → ECB/Frankfurter FX-rate → static — and anchors
//! the quoting mid to the highest live tier. The first two are real market
//! prices and quote healthy; the FX-rate and static tiers are pure peg-rate
//! fallbacks (no live market price), so they run *degraded*, tightening the
//! kill switches in [`super::killswitch`] (§4 row 5). The "full degrade" the
//! spec calls out — every live source down, only the static peg left — is the
//! deepest case of that degraded path.
//!
//! When a market tier is live *and* a fresh FX rate exists, the market price is
//! cross-checked against the fiat peg (`mid / fx_rate ≈ 1` for a sound peg);
//! outside the band that flags a freeze condition. The peg-rate tiers have no
//! independent market price to check against themselves, so they carry no peg.

use crate::config::KillSwitchConfig;
use crate::model::feeds::FeedTier;
use std::time::Duration;

/// One feed reading and how old it is, as of the tick computing `fair_mid`.
#[derive(Clone, Copy, Debug)]
pub struct Quote {
    pub value: f64,
    pub age: Duration,
}

impl Quote {
    pub fn new(value: f64, age: Duration) -> Self {
        Self { value, age }
    }

    /// A reading is usable only while it is younger than the staleness bound.
    fn fresh(&self, stale_after: Duration) -> bool {
        self.age < stale_after && self.value.is_finite() && self.value > 0.0
    }
}

/// Health of the composed reference, gating the hot path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// A live market price (CoinGecko or CoinMarketCap) anchors the mid —
    /// quote normally.
    Ok,
    /// Only a peg-rate fallback is live (the FX-rate or static tier): no live
    /// market price, so quote with the kill switches tightened (§4 row 5: "use
    /// it but flag the vault as degraded"). The spec's "full degrade" — every
    /// live source down to the static peg — is the deepest form of this.
    Degraded,
    /// Nothing usable at all — not even a static peg. Pause `SetReferencePrice`
    /// until a feed returns (§1 / §4). Unreachable while a market configures a
    /// positive `static_usd`, but kept as the defensive floor.
    Pause,
}

/// The composed reference for one tick.
#[derive(Clone, Copy, Debug)]
pub struct FairMid {
    /// The quoting mid (USD per token, human units). `None` only when paused.
    pub mid: Option<f64>,
    /// Which tier produced the mid — surfaced so an operator sees the live
    /// source per market. `None` when paused.
    pub tier: Option<FeedTier>,
    /// Token peg vs the FX rate (`mid / fx_rate`), set only on a market tier
    /// with a fresh FX cross-check.
    pub peg: Option<f64>,
    pub health: Health,
    /// Peg outside `[peg_low, peg_high]` — a freeze condition (§4). Only set
    /// when a market price can be compared to a fresh FX rate.
    pub peg_breach: bool,
}

/// Compose the reference for one market from its tiered readings — `cg`
/// (CoinGecko), `cmc` (CoinMarketCap), and `fx` (the ECB/Frankfurter
/// USD-per-token peg rate), each `None` when that source didn't answer — plus
/// the market's `static_usd` last-resort peg. Cascades primary-first.
pub fn compose(
    cg: Option<Quote>,
    cmc: Option<Quote>,
    fx: Option<Quote>,
    static_usd: f64,
    kill: &KillSwitchConfig,
) -> FairMid {
    let fresh = |q: Option<Quote>| q.filter(|q| q.fresh(kill.feed_stale));
    let cg = fresh(cg);
    let cmc = fresh(cmc);
    let fx = fresh(fx);

    // Cascade primary-first to the highest live tier.
    let picked = if let Some(q) = cg {
        Some((q.value, FeedTier::CoinGecko))
    } else if let Some(q) = cmc {
        Some((q.value, FeedTier::CoinMarketCap))
    } else if let Some(q) = fx {
        Some((q.value, FeedTier::FxRate))
    } else if static_usd.is_finite() && static_usd > 0.0 {
        Some((static_usd, FeedTier::Static))
    } else {
        None
    };

    let Some((mid, tier)) = picked else {
        return FairMid {
            mid: None,
            tier: None,
            peg: None,
            health: Health::Pause,
            peg_breach: false,
        };
    };

    // A live market price quotes healthy; a peg-rate fallback runs degraded.
    let health = if tier.is_market_price() {
        Health::Ok
    } else {
        Health::Degraded
    };

    // Peg sanity only when a market price can be compared to a fresh FX rate;
    // the peg-rate tiers have nothing independent to check against themselves.
    let peg = match (tier.is_market_price(), fx) {
        (true, Some(f)) => Some(mid / f.value),
        _ => None,
    };
    let peg_breach = peg.is_some_and(|p| p < kill.peg_low || p > kill.peg_high);

    FairMid {
        mid: Some(mid),
        tier: Some(tier),
        peg,
        health,
        peg_breach,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kill() -> KillSwitchConfig {
        KillSwitchConfig::default()
    }

    fn q(value: f64) -> Quote {
        Quote::new(value, Duration::from_secs(1))
    }

    fn stale(value: f64) -> Quote {
        Quote::new(value, Duration::from_secs(600))
    }

    #[test]
    fn coingecko_is_the_primary_tier() {
        let r = compose(Some(q(1.14)), Some(q(1.13)), Some(q(1.139)), 1.14, &kill());
        assert_eq!(r.tier, Some(FeedTier::CoinGecko));
        assert_eq!(r.mid, Some(1.14));
        assert_eq!(r.health, Health::Ok);
    }

    #[test]
    fn cascades_to_coinmarketcap_when_coingecko_down() {
        let r = compose(None, Some(q(1.13)), Some(q(1.139)), 1.14, &kill());
        assert_eq!(r.tier, Some(FeedTier::CoinMarketCap));
        assert_eq!(r.mid, Some(1.13));
        assert_eq!(r.health, Health::Ok);
    }

    #[test]
    fn cascades_to_fx_rate_when_markets_down() {
        // No market price: quote off the FX peg, degraded, with no peg check.
        let r = compose(None, None, Some(q(1.139)), 1.14, &kill());
        assert_eq!(r.tier, Some(FeedTier::FxRate));
        assert_eq!(r.mid, Some(1.139));
        assert_eq!(r.health, Health::Degraded);
        assert!(r.peg.is_none());
        assert!(!r.peg_breach);
    }

    #[test]
    fn full_degrade_falls_to_static() {
        // Every live source down → the static peg, the deepest degraded case.
        let r = compose(None, None, None, 1.14, &kill());
        assert_eq!(r.tier, Some(FeedTier::Static));
        assert_eq!(r.mid, Some(1.14));
        assert_eq!(r.health, Health::Degraded);
    }

    #[test]
    fn stale_readings_are_skipped_in_the_cascade() {
        // CoinGecko stale, CMC fresh → CMC carries the mid.
        let r = compose(
            Some(stale(1.14)),
            Some(q(1.13)),
            Some(q(1.139)),
            1.14,
            &kill(),
        );
        assert_eq!(r.tier, Some(FeedTier::CoinMarketCap));
    }

    #[test]
    fn pauses_only_without_a_static_peg() {
        let r = compose(None, None, None, 0.0, &kill());
        assert_eq!(r.health, Health::Pause);
        assert!(r.mid.is_none());
        assert!(r.tier.is_none());
    }

    #[test]
    fn peg_checks_the_market_against_the_fx_rate() {
        // Market 1.14 vs FX peg 1.139 → peg ≈ 1.0009, inside the band.
        let r = compose(Some(q(1.14)), None, Some(q(1.139)), 1.14, &kill());
        assert!(r.peg.is_some());
        assert!(!r.peg_breach);
    }

    #[test]
    fn peg_breach_flags_a_freeze() {
        // Market price 1.25 against an FX peg of 1.139 → peg ≈ 1.097, past the
        // 1.03 ceiling.
        let r = compose(Some(q(1.25)), None, Some(q(1.139)), 1.14, &kill());
        assert!(r.peg_breach);
    }

    #[test]
    fn market_tier_without_fx_still_quotes_ok_without_a_peg() {
        // A live market price with no FX cross-check: quote normally, no peg.
        let r = compose(Some(q(1.14)), None, None, 1.14, &kill());
        assert_eq!(r.health, Health::Ok);
        assert!(r.peg.is_none());
        assert!(!r.peg_breach);
    }
}
