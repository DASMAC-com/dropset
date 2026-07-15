//! One feed reading and its freshness (§1).
//!
//! A [`Reading`] is a single observation of one leg — the FX anchor, the
//! crypto reference, or the USDC/USD peg — carrying its value, how old it is as
//! of the tick that consumes it, and (when the source publishes one) a
//! confidence half-width. Freshness gates whether the reading is usable at all;
//! confidence is a *separate* axis — a fresh reading can still be too uncertain
//! to lean on (§1 failure mode 6: "fresh-but-uncertain" is quote-wider, not
//! do-not-quote).

use std::time::Duration;

/// One feed reading, in its leg's native units (see [`crate::Legs`] for what
/// each leg measures).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// The observed value, in the consuming leg's units.
    pub value: f64,
    /// Age of the reading as of the tick that consumes it. The transport stamps
    /// the read time; the engine compares against the staleness bound.
    pub age: Duration,
    /// Symmetric confidence half-width, in the same units as `value`, when the
    /// source publishes one (Pyth Hermes does; a plain REST quote does not).
    /// `None` means "no confidence notion" — never treated as certain *or*
    /// uncertain, it simply doesn't gate. Drives the fresh-but-uncertain regime
    /// (§1 fm6), never freshness itself.
    pub confidence: Option<f64>,
}

impl Reading {
    /// A reading with no published confidence.
    pub fn new(value: f64, age: Duration) -> Self {
        Self {
            value,
            age,
            confidence: None,
        }
    }

    /// A reading carrying a confidence half-width (e.g. a Pyth price ± conf).
    pub fn with_confidence(value: f64, age: Duration, confidence: f64) -> Self {
        Self {
            value,
            age,
            confidence: Some(confidence),
        }
    }

    /// Usable only while younger than the staleness bound and carrying a
    /// positive, finite value. Everything downstream assumes a fresh reading.
    pub fn fresh(&self, stale_after: Duration) -> bool {
        self.age < stale_after && self.value.is_finite() && self.value > 0.0
    }

    /// Fresh but with a confidence half-width beyond `max_conf_frac` of the
    /// value — the §1 fm6 "quote, but widen the spread" signal. A reading with
    /// no published confidence is never uncertain (the source can't tell us).
    /// This does *not* imply staleness: a wide band is not a dead feed.
    pub fn uncertain(&self, max_conf_frac: f64) -> bool {
        match self.confidence {
            Some(conf) if self.value > 0.0 => conf / self.value > max_conf_frac,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn fresh_within_bound() {
        let r = Reading::new(1.14, secs(1));
        assert!(r.fresh(secs(5)));
    }

    #[test]
    fn stale_past_bound() {
        let r = Reading::new(1.14, secs(10));
        assert!(!r.fresh(secs(5)));
    }

    #[test]
    fn non_positive_and_non_finite_are_never_fresh() {
        assert!(!Reading::new(0.0, secs(1)).fresh(secs(5)));
        assert!(!Reading::new(-1.0, secs(1)).fresh(secs(5)));
        assert!(!Reading::new(f64::NAN, secs(1)).fresh(secs(5)));
        assert!(!Reading::new(f64::INFINITY, secs(1)).fresh(secs(5)));
    }

    #[test]
    fn no_confidence_is_never_uncertain() {
        // A source without a confidence notion can't declare itself uncertain.
        assert!(!Reading::new(1.14, secs(1)).uncertain(0.001));
    }

    #[test]
    fn wide_confidence_is_uncertain_but_still_fresh() {
        // ±0.05 on 1.14 ≈ 4.4% — past a 1% bound → uncertain, yet fresh: a wide
        // band is not a dead feed (§1 fm6).
        let r = Reading::with_confidence(1.14, secs(1), 0.05);
        assert!(r.uncertain(0.01));
        assert!(r.fresh(secs(5)));
    }

    #[test]
    fn tight_confidence_is_certain() {
        // ±0.001 on 1.14 ≈ 0.09% — inside a 1% bound → certain.
        let r = Reading::with_confidence(1.14, secs(1), 0.001);
        assert!(!r.uncertain(0.01));
    }
}
