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

impl Default for FairValueConfig {
    fn default() -> Self {
        Self {
            // Placeholder: the old maker used a flat 5-minute feed staleness.
            // TBD(analytics): split per leg.
            leg_stale: Duration::from_secs(5 * 60),
            // Placeholder: a slow, minutes-scale smoothing so the demo basis
            // (when FX is wired) doesn't chase. TBD(analytics).
            basis_half_life: Duration::from_secs(10 * 60),
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
}
