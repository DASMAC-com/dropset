//! The engine's calibration surface — every constant `fair = fx × basis`
//! consumes, in one place (§1, §4).
//!
//! **Almost every value here is TBD — set by the analytics over collected
//! market-data history (`docs/data-feeds.md` §11).** Until those analytics
//! run, the defaults are *marked placeholders*: chosen wide and demo-safe.
//! The localnet demo runs the full `fair = fx × basis` model live
//! (a Frankfurter FX anchor and a CoinGecko basis leg), so it *does* form a
//! basis — the placeholder bands are set loose enough that a pegged demo token
//! (basis ≈ 1) never trips them, but they are explicitly **not** calibrated
//! for mainnet.
//! Recalibration is a data edit to this one struct, never a code change; each
//! placeholder carries a `TBD(analytics)` marker so the uncalibrated knobs are
//! easy to find.
//!
//! The spec is deliberate that the old fixed `[0.97, 1.03]` basis band and its
//! "300 bps for a Monday gap" rationale were guesses and are **not** reasserted
//! (§1). The placeholder band below is therefore set *wider* than that old
//! guess, so it reads as "demo-safe until calibrated," not as a smuggled-in
//! recalibration.

use std::time::Duration;

/// Every constant the fair-value engine reads. See the module header: the
/// defaults are demo-safe placeholders, not calibrated values.
#[derive(Clone, Copy, Debug)]
pub struct FairValueConfig {
    /// A leg older than this is stale and drops out of the composition.
    /// TBD(analytics): the §1 per-leg staleness thresholds (FX vs basis vs
    /// peg-truth cadences differ by orders of magnitude).
    pub leg_stale: Duration,

    /// The basis EMA smoothing half-life — how slowly the multiplicative
    /// correction tracks its live observations (§1 basis estimation).
    /// TBD(analytics): the smoothing half-life comes from the basis-process
    /// characterization, not guessed here.
    pub basis_half_life: Duration,

    /// Ceiling on the EMA blend weight, so no single observation can replace
    /// the running estimate outright.
    ///
    /// The decay weight `α` rises toward 1 as the gap since the last basis
    /// update grows, which is deliberate — a returning observation after an
    /// outage should re-seed rather than crawl off a stale estimate. Left
    /// uncapped, though, "re-seed" means *one print becomes the basis*: every
    /// FX-session reopen and every anchor-outage recovery is a gap boundary
    /// where a single bad reading determines the whole multiplicative
    /// correction. A reading outside `[basis_low, basis_high]` is caught by the
    /// band, but one *inside* the band multiplies straight into the quoted mid
    /// at full health.
    ///
    /// Capping α keeps the fast re-seed while ensuring the prior estimate
    /// always retains some weight, so a lone outlier is damped rather than
    /// adopted. TBD(analytics): the ceiling follows from the basis-process
    /// characterization and the observed re-seed error distribution.
    pub basis_max_reseed_weight: f64,

    /// Maximum fractional jump from the running estimate that an observation
    /// may make and still be folded, as a fraction of the current estimate.
    ///
    /// This is the pre-fold gate: the band below tests the *smoothed* basis
    /// after the fact, which is too late to stop a bad print from moving it.
    /// An observation further than this from the running estimate is rejected
    /// rather than smoothed, on the reasoning that the basis is by construction
    /// a slow process — a large single-tick move is a bad source, not news.
    /// Applies only once the estimate is seeded; the first observation has
    /// nothing to jump from. TBD(analytics).
    pub basis_max_jump_frac: f64,

    /// How old the carried basis may be before it stops being usable.
    ///
    /// The crate bounds the age of every *input* (`leg_stale`) and, until this
    /// existed, nothing at all bounded the age of its own *state*. In the
    /// FX-live / basis-leg-down degrade the mid is `fx × basis`, so a basis
    /// smoothed six seconds ago and one smoothed five days ago produced
    /// byte-identical output — same regime, same health. Past this bound the
    /// engine stops quoting on the dead estimate and falls to the static-peg or
    /// paused path instead.
    ///
    /// Expiry lives here, next to `leg_stale`, because shared and singly-tested
    /// freshness rules are the point of this crate — exporting the age alone
    /// would repeat the mistake that already had the consumer re-implementing
    /// `Reading::fresh`. TBD(analytics): the bound follows from how fast the
    /// basis process actually drifts.
    pub basis_max_age: Duration,

    /// Per-market sane basis band; a smoothed `basis` outside `[low, high]` is
    /// a basis-band breach → halt (peg event, §4).
    /// TBD(analytics): per-market bands replace this single global placeholder;
    /// the old fixed `[0.97, 1.03]` is deliberately NOT reasserted (§1).
    pub basis_low: f64,
    pub basis_high: f64,

    /// Basis to pin for a market that has **no independent basis source at
    /// all** — `None` for a market whose basis is observed from feeds (the
    /// normal case).
    ///
    /// This is not a fallback and not a calibration knob: it is a statement
    /// that no venue or index prices this token independently of its FX
    /// anchor, so there is nothing to observe. Such a market composes
    /// `fair = fx × pinned_basis` in [`crate::Regime::FxPinned`] and reports
    /// [`crate::Health::Unverified`] — never a basis breach, because a pinned
    /// constant cannot breach a band it was never measured against.
    ///
    /// Set it only when every source tier for the market is absent; feeding a
    /// known-bad source into the basis is worse than admitting there isn't
    /// one, because a permanently-breaching market trains the operator to
    /// ignore the peg alarm (§4).
    pub pinned_basis: Option<f64>,

    /// USDC/USD common-mode band; a USDC/USD reading outside `[low, high]` is a
    /// portfolio-wide common-mode breach → halt every market (§1 fm1, §4).
    /// TBD(analytics).
    pub usdc_low: f64,
    pub usdc_high: f64,

    /// FX confidence half-width, as a fraction of the anchor value, past which
    /// the anchor is *fresh-but-uncertain* — quote, but widen the spread — as
    /// opposed to stale (§1 fm6). TBD(analytics).
    pub fx_max_confidence_frac: f64,
}

/// Why a [`FairValueConfig`] is not usable. Each variant names the invariant
/// the engine relies on and would otherwise trust silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    /// A band's low edge is not strictly below its high edge.
    BandInverted(&'static str),
    /// A value that must be positive and finite is not.
    NotPositive(&'static str),
    /// A fraction that must land in `(0, 1]` does not.
    NotAFraction(&'static str),
    /// A duration that must be non-zero is zero.
    ZeroDuration(&'static str),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BandInverted(field) => write!(f, "{field}: band low edge is not below its high"),
            Self::NotPositive(field) => write!(f, "{field}: must be positive and finite"),
            Self::NotAFraction(field) => write!(f, "{field}: must be a fraction in (0, 1]"),
            Self::ZeroDuration(field) => write!(f, "{field}: must be a non-zero duration"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl FairValueConfig {
    /// Check every invariant the engine relies on but cannot enforce.
    ///
    /// The engine multiplies `pinned_basis` straight into the mid and compares
    /// the smoothed basis against `[basis_low, basis_high]` without testing
    /// either, so a NaN or negative pin yields a NaN or negative `fair` at
    /// `Health::Unverified` with no breach raised, and a zero `basis_half_life`
    /// silently turns the EMA into a pass-through. None of that is reachable
    /// from the compile-time markets table today — this is hardening, so that a
    /// later runtime-configurable path cannot introduce it quietly.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.leg_stale.is_zero() {
            return Err(ConfigError::ZeroDuration("leg_stale"));
        }
        if self.basis_half_life.is_zero() {
            return Err(ConfigError::ZeroDuration("basis_half_life"));
        }
        if self.basis_max_age.is_zero() {
            return Err(ConfigError::ZeroDuration("basis_max_age"));
        }
        if !(self.basis_low.is_finite() && self.basis_high.is_finite())
            || self.basis_low >= self.basis_high
        {
            return Err(ConfigError::BandInverted("basis band"));
        }
        if !(self.usdc_low.is_finite() && self.usdc_high.is_finite())
            || self.usdc_low >= self.usdc_high
        {
            return Err(ConfigError::BandInverted("usdc band"));
        }
        if let Some(pinned) = self.pinned_basis {
            if !pinned.is_finite() || pinned <= 0.0 {
                return Err(ConfigError::NotPositive("pinned_basis"));
            }
        }
        if !is_fraction(self.fx_max_confidence_frac) {
            return Err(ConfigError::NotAFraction("fx_max_confidence_frac"));
        }
        if !is_fraction(self.basis_max_reseed_weight) {
            return Err(ConfigError::NotAFraction("basis_max_reseed_weight"));
        }
        if !is_fraction(self.basis_max_jump_frac) {
            return Err(ConfigError::NotAFraction("basis_max_jump_frac"));
        }
        Ok(())
    }

    /// Layer a market's pinned basis onto this config.
    ///
    /// Pinning is a per-market override of one shared base config, and it was
    /// previously spelled out by hand at each construction site — the live
    /// context builder and the dry-run path each layering it independently,
    /// with only the former covered by a test. A third site would have silently
    /// dropped the pin and quoted the market back on the no-basis-leg degrade
    /// path, a correctness change with no failing test behind it. Routing every
    /// site through this helper makes the layering structural rather than
    /// remembered.
    #[must_use]
    pub fn with_pinned_basis(mut self, pinned: Option<f64>) -> Self {
        self.pinned_basis = pinned;
        self
    }
}

/// Whether `v` is a usable fraction: finite and within `(0, 1]`.
fn is_fraction(v: f64) -> bool {
    v.is_finite() && v > 0.0 && v <= 1.0
}

impl Default for FairValueConfig {
    fn default() -> Self {
        Self {
            // Placeholder: the old maker used a flat 5-minute feed staleness.
            // TBD(analytics): split per leg.
            leg_stale: Duration::from_secs(5 * 60),
            // Placeholder: a slow, minutes-scale smoothing so the demo basis
            // (when FX is wired) doesn't chase. TBD(analytics).
            basis_half_life: Duration::from_secs(10 * 60),
            // Placeholder: a returning observation may take at most 90% of the
            // weight, so the prior estimate always survives a re-seed and a lone
            // outlier is damped rather than adopted. TBD(analytics).
            basis_max_reseed_weight: 0.9,
            // Placeholder: reject an observation more than 5% from the running
            // estimate. Wide enough not to fight the demo, narrow enough that
            // the 0.52×-peg shape that motivated this never folds.
            // TBD(analytics).
            basis_max_jump_frac: 0.05,
            // Placeholder: an hour of carried basis, six times the smoothing
            // half-life above — long enough that a brief basis-leg outage rides
            // through on the last estimate, short enough that the engine never
            // quotes off a dead one. TBD(analytics).
            basis_max_age: Duration::from_secs(60 * 60),
            // Placeholder band, wider than the rejected [0.97, 1.03] guess so it
            // does not masquerade as calibrated. TBD(analytics), per market.
            basis_low: 0.90,
            basis_high: 1.10,
            // A market observes its basis unless its config says no source
            // exists; the engine never pins one on a market's behalf.
            pinned_basis: None,
            // Placeholder USDC/USD common-mode band. TBD(analytics).
            usdc_low: 0.97,
            usdc_high: 1.03,
            // Placeholder: 1% confidence half-width. TBD(analytics).
            fx_max_confidence_frac: 0.01,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bands_are_ordered_and_bracket_one() {
        let c = FairValueConfig::default();
        assert!(c.basis_low < 1.0 && c.basis_high > 1.0);
        assert!(c.usdc_low < 1.0 && c.usdc_high > 1.0);
        assert!(c.basis_low < c.basis_high);
        assert!(c.usdc_low < c.usdc_high);
    }

    #[test]
    fn placeholder_basis_band_is_wider_than_the_rejected_guess() {
        // The spec rejects the old fixed [0.97, 1.03]; the placeholder must not
        // quietly reassert it — it is deliberately wider.
        let c = FairValueConfig::default();
        assert!(c.basis_low < 0.97 && c.basis_high > 1.03);
    }

    #[test]
    fn positive_durations_and_fraction() {
        let c = FairValueConfig::default();
        assert!(c.leg_stale > Duration::ZERO);
        assert!(c.basis_half_life > Duration::ZERO);
        assert!(c.fx_max_confidence_frac > 0.0);
    }

    #[test]
    fn default_validates() {
        assert_eq!(FairValueConfig::default().validate(), Ok(()));
    }

    #[test]
    fn a_zero_half_life_is_rejected() {
        // Left unchecked this turns the EMA into a pass-through, which is
        // exactly the "one print becomes the basis" shape the re-seed cap
        // exists to prevent — so it must not be reachable by configuration.
        let c = FairValueConfig {
            basis_half_life: Duration::ZERO,
            ..Default::default()
        };
        assert_eq!(
            c.validate(),
            Err(ConfigError::ZeroDuration("basis_half_life"))
        );
    }

    #[test]
    fn an_inverted_band_is_rejected() {
        let c = FairValueConfig {
            basis_low: 1.10,
            basis_high: 0.90,
            ..Default::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::BandInverted("basis band")));

        let c = FairValueConfig {
            usdc_low: 1.03,
            usdc_high: 0.97,
            ..Default::default()
        };
        assert_eq!(c.validate(), Err(ConfigError::BandInverted("usdc band")));
    }

    #[test]
    fn a_non_positive_or_non_finite_pin_is_rejected() {
        // The engine multiplies the pin straight into the mid, so a NaN or a
        // negative pin would yield a NaN or negative `fair` reported healthy-ish
        // at `Unverified`, with no breach flag to catch it.
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let c = FairValueConfig::default().with_pinned_basis(Some(bad));
            assert_eq!(
                c.validate(),
                Err(ConfigError::NotPositive("pinned_basis")),
                "pin {bad} should be rejected"
            );
        }
    }

    #[test]
    fn out_of_range_fractions_are_rejected() {
        let c = FairValueConfig {
            basis_max_reseed_weight: 1.5,
            ..Default::default()
        };
        assert_eq!(
            c.validate(),
            Err(ConfigError::NotAFraction("basis_max_reseed_weight"))
        );

        let c = FairValueConfig {
            basis_max_jump_frac: 0.0,
            ..Default::default()
        };
        assert_eq!(
            c.validate(),
            Err(ConfigError::NotAFraction("basis_max_jump_frac"))
        );
    }

    #[test]
    fn pin_layering_is_structural() {
        let base = FairValueConfig::default();
        assert_eq!(base.pinned_basis, None);
        // Layering preserves every other field, which is the property the
        // hand-rolled construction sites had to remember.
        let pinned = base.with_pinned_basis(Some(1.0));
        assert_eq!(pinned.pinned_basis, Some(1.0));
        assert_eq!(pinned.basis_low, base.basis_low);
        assert_eq!(pinned.basis_high, base.basis_high);
        assert_eq!(pinned.leg_stale, base.leg_stale);
        assert_eq!(pinned.basis_half_life, base.basis_half_life);
        assert_eq!(pinned.basis_max_age, base.basis_max_age);
        // And it round-trips back to unpinned.
        assert_eq!(pinned.with_pinned_basis(None).pinned_basis, None);
    }

    #[test]
    fn a_reseed_cap_below_one_leaves_the_prior_some_weight() {
        // The cap is what makes a re-seed a re-seed rather than a replacement;
        // at exactly 1.0 a single post-gap print becomes the basis outright.
        let c = FairValueConfig::default();
        assert!(c.basis_max_reseed_weight < 1.0);
        assert!(c.basis_max_reseed_weight > 0.0);
    }
}
