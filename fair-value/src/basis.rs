//! The stateful basis estimator (§1 basis estimation).
//!
//! `basis` is a **slow, smoothed multiplicative correction** near 1 — an EMA
//! over the live `(token/fiat) ÷ (USDC/USD)` observations, *not* a chased
//! price. The half-life sets how slowly it tracks.
//!
//! This smooths **across time**, and is a different axis from
//! [`crate::Fusion`], which fuses a leg's sources **across one instant**. The
//! two compose rather than duplicating: the basis leg's sources are fused into
//! one reading, and the quotient that reading forms with the FX anchor is what
//! this EMA then smooths. A leg with a single source is untouched by the fusion
//! and still smoothed here.
//!
//! (This module's header used to say a Kalman filter was warranted "only if the
//! bot fuses several basis sources" and deferred it to §5. It now does fuse
//! them — see [`crate::Fusion`]. What remains deferred is the filter learning
//! its own weights, and driving spread width from the estimator's variance.)
//!
//! The decay is derived from the half-life and the *actual* elapsed time
//! between updates, so an irregular tick cadence smooths identically to a
//! regular one: over one half-life the weight on the running estimate halves,
//! whatever the tick spacing.
//!
//! Two guards keep that smoothing from being defeated by a single reading.
//! The decay weight rises toward 1 as the gap since the last update grows —
//! deliberate, so a returning observation re-seeds rather than crawling off a
//! stale estimate — but uncapped it means one print *becomes* the basis at
//! every gap boundary, which is to say every session reopen and every outage
//! recovery. So the weight is **capped below 1**, and an observation too far
//! from the running estimate is **rejected rather than folded**, on the
//! reasoning that the basis is a slow process by construction: a large
//! single-tick move is a bad source, not news.
//!
//! Two things about that rejection rule are load-bearing and easy to get
//! wrong:
//!
//! - **It bounds a single step, not cumulative drift.** The gate measures
//!   against the running estimate — the very thing a sequence of accepted
//!   observations is moving — so a patient source can walk the estimate a long
//!   way in steps that are each individually credible. What stops a slow walk
//!   is the *sane band* on the result, which makes that band load-bearing
//!   rather than merely a peg-event alarm.
//! - **The allowance widens with age, or the estimator never recovers.** A
//!   fixed allowance judges a returning observation against a reference frozen
//!   from before the leg went down; once the true basis has moved further than
//!   that allowance, every returning observation is refused against a stale
//!   reference and the market never comes back. So the gate loosens in step
//!   with how long it has been since anything corroborated the estimate — see
//!   [`BasisEma::jump_allowance`], which also explains why widening is right
//!   and *clearing* the estimate on expiry is not.
//!
//! Rejection is reported, never silent — see [`Fold::Rejected`]. An estimator
//! that quietly dropped readings would turn a sick feed into a frozen basis
//! with nothing to show for it, which is the failure this crate exists to make
//! visible.

use std::time::Duration;

/// Ceiling on how far age may *widen* the jump allowance, as a multiple of the
/// configured base (see [`BasisEma::jump_allowance`]).
///
/// A multiplier rather than an absolute allowance, so that a caller which
/// configures an explicitly unbounded gate still gets one — capping the product
/// would silently re-gate it.
const MAX_JUMP_WIDENING: f64 = 10.0;

/// What happened to an observation offered to the EMA.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fold {
    /// The observation was folded; carries the new smoothed estimate.
    Folded(f64),
    /// The observation was too far from the running estimate to be credible.
    /// The estimate is **unchanged**; both values are carried so the caller can
    /// report which source diverged and by how much.
    Rejected {
        /// The estimate that stands, unmodified.
        estimate: f64,
        /// The observation that was refused.
        observation: f64,
    },
}

impl Fold {
    /// Whether the observation was refused as an outlier.
    pub fn rejected(self) -> bool {
        matches!(self, Self::Rejected { .. })
    }
}

/// EMA of the two-peg basis for one market. Holds the running estimate and the
/// smoothing parameters; the caller feeds it one observation per tick.
#[derive(Clone, Copy, Debug)]
pub struct BasisEma {
    /// Current smoothed basis; `None` until the first observation seeds it.
    value: Option<f64>,
    /// Smoothing half-life (analytics-set — see [`crate::FairValueConfig`]).
    half_life: Duration,
    /// Ceiling on the blend weight, so no single observation replaces the
    /// estimate outright ([`crate::FairValueConfig::basis_max_reseed_weight`]).
    max_weight: f64,
    /// Largest fractional move from the running estimate an observation may
    /// make and still be folded
    /// ([`crate::FairValueConfig::basis_max_jump_frac`]).
    max_jump_frac: f64,
}

impl BasisEma {
    /// A fresh, unseeded estimator with the given smoothing parameters.
    pub fn new(half_life: Duration, max_weight: f64, max_jump_frac: f64) -> Self {
        Self {
            value: None,
            half_life,
            max_weight,
            max_jump_frac,
        }
    }

    /// The current smoothed basis, or `None` before the first observation.
    pub fn value(&self) -> Option<f64> {
        self.value
    }

    /// Offer one basis observation, sampled `dt` after the previous update.
    ///
    /// The first observation seeds the estimate directly — there is nothing to
    /// smooth against and nothing to judge it by, so the seeding tick is
    /// deliberately ungated. Afterwards:
    ///
    /// - an observation further than `max_jump_frac` from the running estimate
    ///   is **rejected**, leaving the estimate untouched;
    /// - otherwise it is folded with weight
    ///   `α = min(1 − 2^(−dt / half_life), max_weight)`. The time-aware part
    ///   means a longer gap weights the new observation more, so the half-life
    ///   means the same thing under any tick cadence; the cap means even an
    ///   unbounded gap leaves the prior estimate some weight.
    ///
    /// A non-finite observation is always rejected. A non-positive or
    /// non-finite `dt`, or a non-positive half-life, collapses the raw weight
    /// to 1 (take the observation as-is) rather than dividing by zero — the cap
    /// then still applies.
    pub fn update(&mut self, observation: f64, dt: Duration) -> Fold {
        let Some(prev) = self.value else {
            // Seeding tick. A non-finite seed would poison every later jump
            // test, so it is refused with nothing to fall back on but itself.
            if !observation.is_finite() {
                return Fold::Rejected {
                    estimate: f64::NAN,
                    observation,
                };
            }
            self.value = Some(observation);
            return Fold::Folded(observation);
        };

        if !observation.is_finite() || self.is_a_jump(prev, observation, dt) {
            return Fold::Rejected {
                estimate: prev,
                observation,
            };
        }

        let alpha = decay_weight(dt, self.half_life).min(self.max_weight);
        let next = prev + alpha * (observation - prev);
        self.value = Some(next);
        Fold::Folded(next)
    }

    /// Whether `observation` sits further from `prev` than an observation
    /// offered `dt` after the last fold is allowed to. A non-positive or
    /// non-finite estimate has no meaningful scale to measure against, so
    /// nothing counts as a jump from it.
    fn is_a_jump(&self, prev: f64, observation: f64, dt: Duration) -> bool {
        if !prev.is_finite() || prev <= 0.0 {
            return false;
        }
        ((observation - prev) / prev).abs() > self.jump_allowance(dt)
    }

    /// How far an observation offered `dt` after the last fold may sit from the
    /// running estimate.
    ///
    /// The allowance **widens with age**, and that is what makes the estimator
    /// recoverable. A fixed allowance measures a returning observation against
    /// a reference frozen at whatever the basis was before the leg went down —
    /// so once the true basis had moved further than the allowance, every
    /// returning observation was refused against a stale reference, forever,
    /// and the market never came back. The gate has to loosen at the same time
    /// the estimate loses authority.
    ///
    /// Widening is deliberately preferred over *clearing* the estimate on
    /// expiry. Clearing looks simpler but is strictly worse: the seeding tick is
    /// ungated by construction, so a source that went quiet for the expiry
    /// window could then seed any value it liked in a single print. Widening
    /// never produces an ungated seed — every observation is still judged, just
    /// against a bound proportionate to how long it has been since anything
    /// was.
    ///
    /// It grows linearly in half-lives elapsed, never tighter than the base
    /// allowance and never wider than [`MAX_JUMP_WIDENING`] times it. Past that
    /// ceiling the gate would stop discriminating; what bounds an estimate
    /// nothing has corroborated for that long is the caller's age bound.
    fn jump_allowance(&self, dt: Duration) -> f64 {
        let hl = self.half_life.as_secs_f64();
        let dt = dt.as_secs_f64();
        if hl <= 0.0 || !dt.is_finite() || dt <= 0.0 {
            return self.max_jump_frac;
        }
        let widening = (dt / hl).clamp(1.0, MAX_JUMP_WIDENING);
        self.max_jump_frac * widening
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

    /// An estimator whose gates are wide open, for the tests that are about the
    /// decay arithmetic rather than the guards. Keeping the guards out of these
    /// tests is deliberate: the decay is a separate property and should still
    /// be pinned exactly, without a gate silently deciding the outcome.
    fn ungated(half_life: Duration) -> BasisEma {
        BasisEma::new(half_life, 1.0, f64::INFINITY)
    }

    /// An estimator with the shipped placeholder gates.
    fn gated(half_life: Duration) -> BasisEma {
        BasisEma::new(half_life, 0.9, 0.05)
    }

    /// Offer `observation` and return the estimate that stands afterwards — the
    /// folded value, or the untouched one if it was refused.
    ///
    /// The estimator deliberately exposes no "the value to quote off" accessor:
    /// whether an estimate is fit to price with depends on its age, which it
    /// does not know. Tests that only care about the arithmetic read the state
    /// back instead.
    fn folded(ema: &mut BasisEma, observation: f64, dt: Duration) -> f64 {
        ema.update(observation, dt);
        ema.value().expect("seeded before this call")
    }

    #[test]
    fn first_observation_seeds_directly() {
        let mut ema = ungated(secs(600));
        assert_eq!(ema.value(), None);
        assert_eq!(ema.update(1.002, secs(5)), Fold::Folded(1.002));
        assert_eq!(ema.value(), Some(1.002));
    }

    #[test]
    fn one_half_life_moves_halfway() {
        // Seed at 1.0, then observe 2.0 exactly one half-life later: the
        // estimate should land halfway, at 1.5.
        let mut ema = ungated(secs(600));
        ema.update(1.0, secs(5));
        let v = folded(&mut ema, 2.0, secs(600));
        assert!((v - 1.5).abs() < 1e-9, "expected 1.5, got {v}");
    }

    #[test]
    fn smoothing_tracks_toward_observations() {
        // A run of observations at a level above the seed pulls the estimate
        // toward that level without overshooting it.
        let mut ema = ungated(secs(600));
        ema.update(1.0, secs(5));
        let mut last = 1.0;
        for _ in 0..200 {
            last = folded(&mut ema, 1.01, secs(30));
        }
        assert!(last > 1.0 && last <= 1.01);
        assert!((last - 1.01).abs() < 1e-3, "should converge near 1.01");
    }

    #[test]
    fn irregular_gaps_weight_more_than_short_ones() {
        // A long gap since the last update should move the estimate further
        // toward the new observation than a short gap would.
        let mut short = ungated(secs(600));
        let mut long = ungated(secs(600));
        short.update(1.0, secs(5));
        long.update(1.0, secs(5));
        let vs = folded(&mut short, 2.0, secs(30));
        let vl = folded(&mut long, 2.0, secs(300));
        assert!(
            vl > vs,
            "longer gap ({vl}) should move more than short ({vs})"
        );
    }

    #[test]
    fn zero_half_life_takes_observation_as_is() {
        let mut ema = ungated(Duration::ZERO);
        ema.update(1.0, secs(5));
        assert_eq!(ema.update(1.5, secs(5)), Fold::Folded(1.5));
    }

    #[test]
    fn a_long_gap_no_longer_lets_one_print_become_the_basis() {
        // This is the defect the cap exists for. A ~60h gap drives the raw
        // decay weight to 1, so before the cap the returning observation landed
        // the estimate on its own raw value. Now the prior estimate keeps
        // `1 - max_weight` of the weight, whatever the gap.
        let mut ema = BasisEma::new(secs(600), 0.9, f64::INFINITY);
        ema.update(1.0, secs(5));
        let v = folded(&mut ema, 1.5, secs(60 * 60 * 60));
        assert!(v < 1.5, "a single post-gap print must not become the basis");
        assert!((v - 1.45).abs() < 1e-9, "expected 1.0 + 0.9*0.5, got {v}");
    }

    #[test]
    fn the_cap_binds_only_at_long_gaps() {
        // At short gaps the raw decay weight is well under the cap, so capping
        // must not perturb ordinary smoothing at all.
        let mut capped = BasisEma::new(secs(600), 0.9, f64::INFINITY);
        let mut uncapped = ungated(secs(600));
        capped.update(1.0, secs(5));
        uncapped.update(1.0, secs(5));
        let c = folded(&mut capped, 1.01, secs(30));
        let u = folded(&mut uncapped, 1.01, secs(30));
        assert!((c - u).abs() < 1e-12, "cap should not bind at a 30s gap");
    }

    #[test]
    fn an_observation_beyond_the_jump_gate_is_rejected() {
        // The shape that motivated this: an aggregate reading roughly half the
        // peg arriving against a healthy estimate near 1.
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        let fold = ema.update(0.52, secs(30));
        assert_eq!(
            fold,
            Fold::Rejected {
                estimate: 1.0,
                observation: 0.52
            }
        );
        assert!(fold.rejected());
        // The estimate is untouched — a refused observation leaves no trace.
        assert_eq!(ema.value(), Some(1.0));
    }

    #[test]
    fn an_observation_inside_the_jump_gate_still_folds() {
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        let fold = ema.update(1.02, secs(30));
        assert!(!fold.rejected(), "2% is inside a 5% gate");
        assert!(ema.value().unwrap() > 1.0);
    }

    #[test]
    fn the_jump_gate_does_not_apply_to_the_seeding_tick() {
        // There is no estimate to judge the first observation against, so the
        // seeding tick is ungated by construction. A market whose true basis is
        // far from 1 must still be able to seed there.
        let mut ema = gated(secs(600));
        assert_eq!(ema.update(0.52, secs(5)), Fold::Folded(0.52));
        assert_eq!(ema.value(), Some(0.52));
    }

    #[test]
    fn a_persistent_shift_is_refused_at_the_prevailing_cadence() {
        // A real regime change and a stuck bad source look identical to the
        // gate at a given cadence, so it refuses both. That is the intended
        // tradeoff at this timescale: the caller sees a run of rejections rather
        // than the estimator quietly walking to a wrong level.
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        for _ in 0..50 {
            assert!(ema.update(0.52, secs(30)).rejected());
        }
        assert_eq!(ema.value(), Some(1.0));
    }

    #[test]
    fn a_refused_shift_is_eventually_accepted_as_the_estimate_ages() {
        // The other half of the rule above, and the one the estimator used to
        // lack: refusal must not be permanent. Offered far enough after the last
        // fold, a moved basis is credible precisely because there has been time
        // for it to move — so the gate widens and the estimate recovers without
        // a restart.
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        // Immediately, a 20% move is not credible.
        assert!(ema.update(1.20, secs(30)).rejected());
        // Four half-lives later, it is: the allowance has widened to 20%.
        assert!(!ema.update(1.20, secs(4 * 600)).rejected());
        assert!(ema.value().unwrap() > 1.0);
    }

    #[test]
    fn the_widened_allowance_never_becomes_a_free_pass() {
        // Widening must not degenerate into the ungated seed that clearing the
        // estimate would have produced: however long the gap, an observation
        // still has to be judged against something.
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        // A century of silence does not license an arbitrary value.
        assert!(ema.update(1_000.0, secs(100 * 365 * 24 * 3_600)).rejected());
        assert_eq!(ema.value(), Some(1.0));
    }

    #[test]
    fn the_allowance_is_never_tighter_than_the_base_gate() {
        // A sub-half-life cadence — the normal case — must not be gated tighter
        // than the configured fraction just because little time has passed.
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        // 4% at a 1-second cadence is inside the 5% base allowance.
        assert!(!ema.update(1.04, secs(1)).rejected());
    }

    #[test]
    fn non_finite_observations_are_refused() {
        let mut ema = gated(secs(600));
        ema.update(1.0, secs(5));
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(ema.update(bad, secs(30)).rejected(), "{bad} should refuse");
        }
        assert_eq!(ema.value(), Some(1.0));
    }

    #[test]
    fn a_non_finite_seed_does_not_poison_the_estimator() {
        let mut ema = gated(secs(600));
        assert!(ema.update(f64::NAN, secs(5)).rejected());
        assert_eq!(ema.value(), None, "a refused seed leaves it unseeded");
        // And a good reading afterwards still seeds cleanly.
        assert_eq!(ema.update(1.0, secs(5)), Fold::Folded(1.0));
    }
}
