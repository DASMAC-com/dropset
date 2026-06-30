//! Reference-price composition and the peg sanity bound (§1).
//!
//! `fair_mid` is the mean of the two CADC market-price sources (CoinGecko and
//! Aerodrome) — *not* Oanda, which measures the underlying CAD/USD FX rate and
//! serves only as a peg-deviation sanity check. The composition also folds in
//! the §1 / §4 freshness rules: disagreeing or stale CADC sources pause the
//! reference hot path, *any* single stale feed (one CADC source, or the FX
//! feed) runs degraded, and a peg outside the band flags a freeze condition.
//! Degraded tightens the kill switches by 50% (§4 row 5) — the peg band halves
//! its margin here, the imbalance bounds and TVL floor tighten in
//! [`super::killswitch`].

use crate::config::KillSwitchConfig;
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
    /// Both CADC sources fresh and in agreement — quote normally.
    Ok,
    /// A single feed is stale — one CADC source out (the other carries the
    /// mid) or the FX feed out (the CADC mid still quotes, but the peg switch
    /// disarms). Quote with the kill switches tightened (§1 / §4 row 5: "use
    /// it but flag the vault as degraded").
    Degraded,
    /// CADC sources disagree past the bound, or none is fresh — pause
    /// `SetReferencePrice` until resolved (§1 / §4).
    Pause,
}

/// The composed reference for one tick.
#[derive(Clone, Copy, Debug)]
pub struct FairMid {
    /// The quoting mid (mean of fresh CADC sources). `None` when paused.
    pub mid: Option<f64>,
    /// CADC peg vs FX spot (`mid / f_fx`), when a fresh FX reading exists.
    pub peg: Option<f64>,
    pub health: Health,
    /// Peg outside `[peg_low, peg_high]` — a freeze condition (§4). Only set
    /// when both a mid and a fresh FX reading exist; Oanda staleness merely
    /// disarms this switch, it never trips it.
    pub peg_breach: bool,
}

/// The peg sanity band `[low, high]` for this tick. Healthy it is the
/// configured `[peg_low, peg_high]`; degraded it halves each margin around the
/// band's midpoint (§4 row 5), e.g. `[0.97, 1.03]` → `[0.985, 1.015]`.
fn peg_band(kill: &KillSwitchConfig, degraded: bool) -> (f64, f64) {
    if !degraded {
        return (kill.peg_low, kill.peg_high);
    }
    let mid = (kill.peg_low + kill.peg_high) / 2.0;
    (
        mid + (kill.peg_low - mid) * 0.5,
        mid + (kill.peg_high - mid) * 0.5,
    )
}

/// Compose `fair_mid` from the CADC sources (`cg` = CoinGecko, `ae` =
/// Aerodrome, the latter `None` when the feed is disabled) and the Oanda FX
/// reading (`fx`). `None` arguments are absent feeds.
pub fn compose(
    cg: Option<Quote>,
    ae: Option<Quote>,
    fx: Option<Quote>,
    kill: &KillSwitchConfig,
) -> FairMid {
    let fresh = |q: Option<Quote>| q.filter(|q| q.fresh(kill.feed_stale));
    let cg = fresh(cg);
    let ae = fresh(ae);
    let fx = fresh(fx);

    let (mid, mut health) = match (cg, ae) {
        // Both CADC sources fresh: pause if they disagree, else mean them.
        (Some(a), Some(b)) => {
            let mean = (a.value + b.value) / 2.0;
            let disagree_bps = (a.value - b.value).abs() / mean * 10_000.0;
            if disagree_bps > kill.cadc_disagree_bps {
                (None, Health::Pause)
            } else {
                (Some(mean), Health::Ok)
            }
        }
        // Exactly one fresh CADC source: usable, but degraded.
        (Some(q), None) | (None, Some(q)) => (Some(q.value), Health::Degraded),
        // No fresh CADC source: nothing to quote against.
        (None, None) => (None, Health::Pause),
    };

    // A stale (or absent) FX feed is also a single stale feed (§4 row 5): the
    // CADC mid still quotes, but the vault runs degraded. Never upgrades a
    // paused reference — only a healthy one steps down to degraded.
    if fx.is_none() && health == Health::Ok {
        health = Health::Degraded;
    }

    // Degraded halves the peg band's margin around par (§4 row 5), so a fresh
    // peg reading trips the freeze switch on half the deviation.
    let (peg_low, peg_high) = peg_band(kill, health == Health::Degraded);
    let peg = match (mid, fx) {
        (Some(m), Some(f)) => Some(m / f.value),
        _ => None,
    };
    let peg_breach = peg.is_some_and(|p| p < peg_low || p > peg_high);

    FairMid {
        mid,
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

    #[test]
    fn means_two_agreeing_cadc_sources() {
        let r = compose(Some(q(0.73)), Some(q(0.7302)), Some(q(0.73)), &kill());
        assert_eq!(r.health, Health::Ok);
        assert!((r.mid.unwrap() - 0.7301).abs() < 1e-9);
    }

    #[test]
    fn pauses_when_cadc_sources_disagree() {
        // 0.73 vs 0.75 is ~270 bps apart, past the 50 bps bound.
        let r = compose(Some(q(0.73)), Some(q(0.75)), Some(q(0.73)), &kill());
        assert_eq!(r.health, Health::Pause);
        assert!(r.mid.is_none());
    }

    #[test]
    fn single_fresh_cadc_source_runs_degraded() {
        let r = compose(Some(q(0.73)), None, Some(q(0.73)), &kill());
        assert_eq!(r.health, Health::Degraded);
        assert_eq!(r.mid, Some(0.73));
    }

    #[test]
    fn pauses_when_all_cadc_sources_stale() {
        let stale = Quote::new(0.73, Duration::from_secs(600));
        let r = compose(Some(stale), Some(stale), Some(q(0.73)), &kill());
        assert_eq!(r.health, Health::Pause);
    }

    #[test]
    fn peg_breach_flags_a_freeze() {
        // CADC at 0.80 against FX 0.73 → peg 1.096, outside the 1.03 ceiling.
        let r = compose(Some(q(0.80)), Some(q(0.80)), Some(q(0.73)), &kill());
        assert!(r.peg_breach);
    }

    #[test]
    fn stale_oanda_disarms_peg_switch_and_runs_degraded() {
        let stale_fx = Quote::new(0.50, Duration::from_secs(600));
        let r = compose(Some(q(0.73)), Some(q(0.73)), Some(stale_fx), &kill());
        assert!(r.peg.is_none());
        assert!(!r.peg_breach);
        // CADC sources are still fine, so quoting continues — but a stale FX
        // feed is a single stale feed, so the vault runs degraded (§4 row 5).
        assert_eq!(r.health, Health::Degraded);
    }

    #[test]
    fn degraded_tightens_the_peg_band() {
        // CADC 0.745 against FX 0.73 → peg ~1.021: inside the healthy
        // [0.97, 1.03] band, but outside the degraded [0.985, 1.015] band once
        // a single CADC source drops the vault to degraded.
        let healthy = compose(Some(q(0.745)), Some(q(0.745)), Some(q(0.73)), &kill());
        assert_eq!(healthy.health, Health::Ok);
        assert!(!healthy.peg_breach);

        let degraded = compose(Some(q(0.745)), None, Some(q(0.73)), &kill());
        assert_eq!(degraded.health, Health::Degraded);
        assert!(degraded.peg_breach);
    }
}
