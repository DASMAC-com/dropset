//! The stateful basis estimator (§1 basis estimation).
//!
//! `basis` is a **slow, smoothed multiplicative correction** near 1 — an EMA
//! over the live `(token/fiat) ÷ (USDC/USD)` observations, *not* a chased
//! price. The half-life sets how slowly it tracks. A Kalman filter (fusing
//! several basis sources, or driving spread width from the basis variance) is
//! warranted only later and is **deferred to §5**.
//!
//! The decay is derived from the half-life and the *actual* elapsed time
//! between updates, so an irregular tick cadence smooths identically to a
//! regular one: over one half-life the weight on the running estimate halves,
//! whatever the tick spacing.

use std::time::Duration;

/// EMA of the two-peg basis for one market. Holds only the running estimate and
/// the half-life; the caller feeds it one observation per tick.
#[derive(Clone, Copy, Debug)]
pub struct BasisEma {
    /// Current smoothed basis; `None` until the first observation seeds it.
    value: Option<f64>,
    /// Smoothing half-life (survey-set — see [`crate::FairValueConfig`]).
    half_life: Duration,
}

impl BasisEma {
    /// A fresh, unseeded estimator with the given smoothing half-life.
    pub fn new(half_life: Duration) -> Self {
        Self {
            value: None,
            half_life,
        }
    }

    /// The current smoothed basis, or `None` before the first observation.
    pub fn value(&self) -> Option<f64> {
        self.value
    }

    /// Fold one basis observation, sampled `dt` after the previous update, into
    /// the EMA, and return the new smoothed value. The first observation seeds
    /// the estimate directly (no smoothing to apply yet).
    ///
    /// The blend weight `α = 1 − 2^(−dt / half_life)` is time-aware: a longer
    /// gap since the last update weights the new observation more, so the
    /// half-life means the same thing under any tick cadence. A non-positive or
    /// non-finite `dt`, or a non-positive half-life, collapses to `α = 1` (take
    /// the observation as-is) rather than dividing by zero.
    pub fn update(&mut self, observation: f64, dt: Duration) -> f64 {
        let next = match self.value {
            None => observation,
            Some(prev) => {
                let alpha = decay_weight(dt, self.half_life);
                prev + alpha * (observation - prev)
            }
        };
        self.value = Some(next);
        next
    }

    /// Forget the running estimate — used when a market's FX anchor drops so
    /// long that the last basis is no longer meaningful, so the next live tick
    /// re-seeds rather than blending onto a stale estimate.
    pub fn reset(&mut self) {
        self.value = None;
    }
}

/// The EMA blend weight for an elapsed `dt` and a smoothing `half_life`:
/// `1 − 2^(−dt / half_life)`, clamped into `[0, 1]`. Degenerate inputs (a
/// non-finite or non-positive `dt`, or a non-positive half-life) yield `1.0`.
fn decay_weight(dt: Duration, half_life: Duration) -> f64 {
    let hl = half_life.as_secs_f64();
    let dt = dt.as_secs_f64();
    if hl <= 0.0 || !dt.is_finite() || dt <= 0.0 {
        return 1.0;
    }
    let alpha = 1.0 - (-(dt / hl) * std::f64::consts::LN_2).exp();
    alpha.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn first_observation_seeds_directly() {
        let mut ema = BasisEma::new(secs(600));
        assert_eq!(ema.value(), None);
        let v = ema.update(1.002, secs(5));
        assert_eq!(v, 1.002);
        assert_eq!(ema.value(), Some(1.002));
    }

    #[test]
    fn one_half_life_moves_halfway() {
        // Seed at 1.0, then observe 2.0 exactly one half-life later: the
        // estimate should land halfway, at 1.5.
        let mut ema = BasisEma::new(secs(600));
        ema.update(1.0, secs(5));
        let v = ema.update(2.0, secs(600));
        assert!((v - 1.5).abs() < 1e-9, "expected 1.5, got {v}");
    }

    #[test]
    fn smoothing_tracks_toward_observations() {
        // A run of observations at a level above the seed pulls the estimate
        // toward that level without overshooting it.
        let mut ema = BasisEma::new(secs(600));
        ema.update(1.0, secs(5));
        let mut last = 1.0;
        for _ in 0..200 {
            last = ema.update(1.01, secs(30));
        }
        assert!(last > 1.0 && last <= 1.01);
        assert!((last - 1.01).abs() < 1e-3, "should converge near 1.01");
    }

    #[test]
    fn irregular_gaps_weight_more_than_short_ones() {
        // A long gap since the last update should move the estimate further
        // toward the new observation than a short gap would.
        let mut short = BasisEma::new(secs(600));
        let mut long = BasisEma::new(secs(600));
        short.update(1.0, secs(5));
        long.update(1.0, secs(5));
        let vs = short.update(2.0, secs(30));
        let vl = long.update(2.0, secs(300));
        assert!(vl > vs, "longer gap ({vl}) should move more than short ({vs})");
    }

    #[test]
    fn zero_half_life_takes_observation_as_is() {
        let mut ema = BasisEma::new(Duration::ZERO);
        ema.update(1.0, secs(5));
        assert_eq!(ema.update(1.5, secs(5)), 1.5);
    }

    #[test]
    fn reset_re_seeds() {
        let mut ema = BasisEma::new(secs(600));
        ema.update(1.2, secs(5));
        ema.reset();
        assert_eq!(ema.value(), None);
        assert_eq!(ema.update(0.9, secs(5)), 0.9);
    }
}
