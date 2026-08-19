//! The fair-value composition — `fair = fx × basis` and its regimes (§1).
//!
//! Per market, per tick, the engine composes one mid from two legs: a fast,
//! deep, exogenous **FX anchor** corrected by a slow, thin **basis**. Which
//! legs are live selects the *regime* — a first-class state, not an exception
//! (§1 "regimes and failure modes"):
//!
//! - **Normal** — FX anchor *and* crypto reference live. `basis` is the EMA of
//!   the observed `crypto_usdc / fx`, and `fair = fx × basis`.
//! - **Crypto-only** — no live FX. On a weekend/session close (§1 fm2) this is
//!   the *normal* state: interbank FX is shut, so the crypto reference *is* the
//!   only price discovery and becomes the anchor — healthy, not degraded. This
//!   regime fires only when the FX anchor is a *session* feed that goes absent
//!   off-session (the streaming primary). With a daily-reference anchor that
//!   serves frozen weekend rates (the current Frankfurter fallback) the FX leg
//!   stays present, so the flip is forward-plumbing for the streaming anchor.
//! - **Degraded** — an *unexpected* gap: FX stale outside the weekend regime
//!   (crypto reference carries the mid, kill switches tighten, §4), or FX up
//!   but the basis leg down (anchor on the last smoothed basis), or every live
//!   leg down to the static peg (the deepest degraded case).
//! - **Paused** — nothing usable, not even a static peg. The caller stops
//!   quoting until a leg returns.
//!
//! Two guard signals ride alongside the mid, independent of the regime:
//! **basis-band breach** (a smoothed basis outside its sane band → halt, a peg
//! event) and the **USDC/USD common-mode breach** (a USDC depeg moves every
//! market's basis at once → a portfolio-wide halt, §1 fm1). The engine only
//! *raises* them; mapping a breach to an action, and lifting the common-mode
//! breach across the whole portfolio, is the caller's kill-switch policy (§4).

use std::time::Duration;

use crate::basis::{BasisEma, Fold};
use crate::config::FairValueConfig;
use crate::reading::Reading;

/// The raw feed legs for one market on one tick, each `None` when its source
/// didn't answer. Units differ per leg and matter — see each field (§1).
#[derive(Clone, Copy, Debug, Default)]
pub struct Legs {
    /// FX anchor: the fiat cross as **USD per fiat unit** (EUR/USD ≈ 1.14 for
    /// EURC). Deep, exogenous, streamed (Pyth Hermes / OANDA). `None` off
    /// session (the weekend regime) or on an outage. May carry a confidence
    /// half-width (§1 fm6).
    pub fx: Option<Reading>,
    /// Crypto reference: the token priced directly in **USDC per token** on a
    /// crypto venue (Coinbase `<token>/USDC`; CoinGecko/CMC token-USD as the
    /// last-resort fallback). Two roles — the numerator of the observed basis
    /// (`crypto_usdc / fx`) in the normal regime, and the anchor itself in the
    /// crypto-only regime.
    pub crypto_usdc: Option<Reading>,
    /// USDC/USD peg truth as **USD per USDC** — a *separate* USDC anchor
    /// (Coinbase USDC/USD, Circle redemption). Drives the portfolio-wide
    /// common-mode guard (§1 fm1). `None` simply means the guard can't fire
    /// this tick; the basis is still observable from `crypto_usdc / fx`.
    pub usdc_usd: Option<Reading>,
    /// Last-resort static USD-per-token peg (a config constant, not a feed).
    /// Used only when every live leg is down — the deepest degraded case.
    pub static_usd: f64,
}

/// The wall-clock context for one tick — facts about *when* the tick is, as
/// opposed to what the feeds said.
///
/// A struct rather than a bare `weekend: bool` because session state is not the
/// only clock fact the composition needs: proximity to a scheduled macro
/// release is the same kind of input (global, not per-leg, derived from the
/// clock rather than observed), and belongs beside this one rather than as
/// another positional bool.
#[derive(Clone, Copy, Debug, Default)]
pub struct ClockCtx {
    /// The FX session is closed (§1 fm2), inside which an absent FX anchor is
    /// the structural crypto-only state rather than a degrade.
    pub weekend: bool,
}

impl ClockCtx {
    /// A context for a tick inside the open FX session.
    pub fn in_session() -> Self {
        Self { weekend: false }
    }

    /// A context for a tick in the FX-closed window.
    pub fn weekend() -> Self {
        Self { weekend: true }
    }
}

impl Legs {
    /// The two-peg basis these legs imply, when it is observable at all: both
    /// legs present and fresh, and the FX anchor a usable divisor. `None` means
    /// there is nothing to observe this tick — a source is still warming, or one
    /// has dropped out.
    ///
    /// Lives here rather than in the consumer so the gating rule and the
    /// division are stated once. The consumer's copy had already drifted into
    /// re-implementing the crate's leg gating alongside the arithmetic, which is
    /// the shape this crate exists to prevent.
    pub fn observed_basis(&self, stale: Duration) -> Option<f64> {
        let fx = self.fx.filter(|r| r.fresh(stale))?;
        let crypto = self.crypto_usdc.filter(|r| r.fresh(stale))?;
        observed_basis(crypto.value, fx.value)
    }
}

/// Which leg is anchoring the mid this tick — surfaced per market for the
/// operator (§1 "the bot surfaces which source is live per leg").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// The exogenous FX cross (the normal-regime anchor).
    Fx,
    /// The crypto reference standing in as the anchor (weekend/session, or an
    /// unexpected FX outage).
    CryptoReference,
    /// The static configured peg (deepest degrade).
    Static,
    /// Nothing anchors the mid — paused.
    None,
}

/// The composition regime for the tick (§1). Distinguishes a *structural*
/// crypto-only window (weekend) from an *unexpected* degrade, which the old
/// cascade conflated as "fall back to a lower tier."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Regime {
    /// FX anchor × basis — both legs live.
    Normal,
    /// Crypto reference is the anchor by design (FX session closed, §1 fm2).
    CryptoOnly,
    /// FX anchor × a **pinned** basis, because the market has no independent
    /// basis source to observe one from ([`crate::FairValueConfig::pinned_basis`]).
    ///
    /// Structural, like [`Regime::CryptoOnly`] and unlike [`Regime::Degraded`]:
    /// nothing has gone wrong and nothing will recover, so it is not a degrade.
    /// The distinction is the same one this enum already draws between a
    /// weekend and an outage — a permanent state reported as a fault is a
    /// fault the operator learns to ignore.
    FxPinned,
    /// A degrade — see [`Degrade`] for which.
    Degraded(Degrade),
    /// No usable leg at all.
    Paused,
}

impl Regime {
    /// The health gate this regime implies (§4).
    ///
    /// Health is a **total function of the regime** — there is no arm in which
    /// they disagree — so it is derived here rather than restated at each
    /// composition site. It stays a public field on [`FairValue`] because it is
    /// the documented kill-switch axis and is constructed independently by
    /// consumers; what this removes is the possibility of the two drifting,
    /// where a new degraded arm could set the regime and forget the health,
    /// silently un-tightening every kill switch while the operator log still
    /// read "degraded".
    pub fn health(self) -> Health {
        match self {
            Self::Normal | Self::CryptoOnly => Health::Ok,
            Self::FxPinned => Health::Unverified,
            Self::Degraded(_) => Health::Degraded,
            Self::Paused => Health::Pause,
        }
    }
}

/// Why the engine is degraded (§1 "degraded and halt conditions", §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degrade {
    /// FX anchor stale or absent outside the weekend regime — the crypto
    /// reference carries the mid, but this is an unexpected gap, so run
    /// degraded (§4).
    FxStale,
    /// FX anchor answering promptly with an unusable value — non-finite, zero,
    /// or negative.
    ///
    /// Distinguished from [`Degrade::FxStale`] because the two call for
    /// different operator responses and used to be indistinguishable: a dead
    /// poller and a closed session are staleness, while this is a live feed
    /// publishing garbage, which is a wiring or upstream-format problem. Both
    /// degrade identically — every folded case is equally unusable — so this
    /// changes what the operator is told, not what the bot does.
    FxInvalid,
    /// FX anchor live but the crypto basis leg is down — anchor on FX with the
    /// last smoothed basis, on thinner information.
    NoBasisLeg,
    /// FX anchor live, but the smoothed basis is **unusable**: never seeded, or
    /// carried past [`crate::FairValueConfig::basis_max_age`].
    ///
    /// `fair = fx × basis` cannot be composed without a basis, and the engine
    /// declines to invent one. It previously substituted `1.0` here, which is a
    /// fabricated parity claim indistinguishable in the output from an observed
    /// basis of exactly 1 — so a market whose basis had never been observed at
    /// all quoted as though it had been measured and found at par. The mid
    /// falls to the static peg instead (or pauses if there is none), which is
    /// the same treatment any other unusable leg gets.
    BasisUnusable,
    /// Every live leg down to the static peg — the deepest degraded case.
    StaticPeg,
}

/// Health gate for the quoting hot path — the axis the kill-switch policy reads
/// (§4). `Degraded` tightens the switches by 50%; `Pause` stops quoting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Quote normally.
    Ok,
    /// Quote normally, but on a reference **no independent source corroborates**
    /// — the market's basis is pinned rather than observed ([`Regime::FxPinned`]).
    ///
    /// Deliberately neither `Ok` nor `Degraded`. Not `Ok`, because the mid rests
    /// on one unchecked leg and the operator is entitled to see that. Not
    /// `Degraded`, because the kill switches tighten on a degrade and this state
    /// is permanent — quoting such a market at half width forever is a standing
    /// cost paid for information that will never arrive, and it re-creates the
    /// very desensitization this variant exists to prevent.
    Unverified,
    /// Quote with the kill switches tightened (§4 row: FX stale → degrade).
    Degraded,
    /// Do not quote — no usable reference.
    Pause,
}

/// The composed reference for one market for one tick.
#[derive(Clone, Copy, Debug)]
pub struct FairValue {
    /// The quoting mid, in USDC per token (human units). `None` only when
    /// [`Regime::Paused`].
    pub fair: Option<f64>,
    /// Which leg anchored the mid.
    pub anchor: Anchor,
    /// The composition regime.
    pub regime: Regime,
    /// The smoothed basis, set only in an FX-anchored regime (there is no basis
    /// without an FX anchor to divide the crypto reference by).
    pub basis: Option<f64>,
    /// How long ago the basis in `basis` was last *observed* — zero on a tick
    /// that folded an observation, growing on every tick that did not.
    ///
    /// Without this a basis smoothed six seconds ago and one smoothed five days
    /// ago produced identical output, so the operator could not tell a live
    /// correction from a carried one. `None` whenever `basis` is `None`, and
    /// for a pinned basis, which is a constant and has no observation age.
    pub basis_age: Option<Duration>,
    /// The basis leg answered this tick, but its reading was refused as an
    /// outlier rather than folded (see [`crate::Fold::Rejected`]).
    ///
    /// Distinct from `basis_breach`: a breach says the *smoothed* basis has left
    /// its sane band, while this says a *single* reading was too far from the
    /// running estimate to be credible. A run of these is what a sick source
    /// looks like before it has moved anything.
    pub basis_outlier: bool,
    /// Health gate for the kill switches.
    pub health: Health,
    /// The FX anchor is fresh but too uncertain (§1 fm6) — quote, but the
    /// caller should widen the spread. Never set in a non-FX regime.
    pub uncertain: bool,
    /// The smoothed basis is outside its sane band — a peg event → halt (§4).
    /// Only meaningful when `basis` is `Some`.
    pub basis_breach: bool,
    /// The USDC/USD reading is outside its common-mode band — a portfolio-wide
    /// event → halt every market (§1 fm1, §4). Evaluated whenever a USDC/USD
    /// reading is live, in *any* regime.
    pub usdc_breach: bool,
}

impl FairValue {
    /// The skeleton of a composition: the regime, what anchors it, and the mid,
    /// with every guard signal off and no basis. Each arm fills in what it
    /// actually observed via struct-update syntax.
    ///
    /// `health` is derived here and never passed, which is the whole point —
    /// see [`Regime::health`].
    fn of(regime: Regime, anchor: Anchor, fair: Option<f64>) -> Self {
        Self {
            fair,
            anchor,
            regime,
            basis: None,
            basis_age: None,
            health: regime.health(),
            uncertain: false,
            basis_breach: false,
            basis_outlier: false,
            usdc_breach: false,
        }
    }

    /// Whether the kill-switch policy should run tightened (§4). True in every
    /// degraded regime; false when healthy, paused, or
    /// [`Health::Unverified`] — see that variant for why a permanently
    /// uncorroborated market is not quoted tightened.
    pub fn degraded(&self) -> bool {
        self.health == Health::Degraded
    }
}

/// Per-market fair-value engine: the calibration constants plus the stateful
/// basis EMA. One instance per market (each carries its own basis history).
/// Not `Copy`: the basis EMA is a mutating accumulator, and a silent
/// copy-on-assign would fork that history.
#[derive(Clone, Debug)]
pub struct FairValueEngine {
    cfg: FairValueConfig,
    basis: BasisEma,
    /// Time accumulated across ticks since the basis EMA was last updated. The
    /// EMA folds an observation only in the normal regime, but `compose` runs
    /// every tick — so the decay must see the elapsed time since the last
    /// *basis update*, not since the last `compose`. Accumulating here means a
    /// long gap with no live basis (an FX outage) yields a blend weight near 1
    /// on the returning observation — a natural re-seed rather than a crawl
    /// off a stale estimate.
    since_basis: Duration,
}

impl FairValueEngine {
    /// Build an engine for one market from its calibration config.
    pub fn new(cfg: FairValueConfig) -> Self {
        Self {
            basis: BasisEma::new(
                cfg.basis_half_life,
                cfg.basis_max_reseed_weight,
                cfg.basis_max_jump_frac,
            ),
            since_basis: Duration::ZERO,
            cfg,
        }
    }

    /// Compose the fair value for this market from its live `legs`. `dt` is the
    /// elapsed time since the previous `compose`; the engine accumulates it so
    /// the basis EMA sees the time since the last basis update. `weekend` marks
    /// the FX-closed session window (§1 fm2), inside which FX-stale is the
    /// normal crypto-only state rather than a degrade.
    pub fn compose(&mut self, legs: Legs, dt: Duration, clock: ClockCtx) -> FairValue {
        let stale = self.cfg.leg_stale;
        // Carry the inter-tick time forward; the normal arm consumes it and
        // resets it when it actually folds an observation into the EMA.
        self.since_basis = self.since_basis.saturating_add(dt);
        let fx = legs.fx.filter(|r| r.fresh(stale));
        let usdc = legs.usdc_usd.filter(|r| r.fresh(stale));

        // A pinned market has **no independent basis source by definition**, so
        // any crypto reading present for it is not one — drop it here, before
        // anything can read it. Previously this was implicit in a nested
        // `if let` that returned only on the FX-live path, leaving a pinned
        // market with a live crypto leg and a down FX anchor to fall through to
        // the shared match and price off `crypto.value` — the exact reading the
        // pin exists to declare unusable. No configuration could reach that
        // (the pin comes from a compile-time table whose pinned entries carry no
        // basis-source ids), so this asserts an invariant the crate previously
        // only trusted its caller to maintain.
        let crypto = legs
            .crypto_usdc
            .filter(|_| self.cfg.pinned_basis.is_none())
            .filter(|r| r.fresh(stale));

        // The USDC/USD common-mode guard is regime-independent: a depeg moves
        // every market's basis at once, so it is evaluated wherever a live
        // USDC/USD reading exists (§1 fm1).
        let usdc_breach =
            usdc.is_some_and(|u| u.value < self.cfg.usdc_low || u.value > self.cfg.usdc_high);

        // `since_basis` accumulates here and is only reset when the EMA folds an
        // observation, which a pinned market never reaches. That is harmless
        // rather than a leak: it saturates instead of overflowing, and its sole
        // reader is the EMA decay in the normal arm — unreachable for a pinned
        // market, whose config invariant is that it has no crypto source to pair
        // with the anchor.
        //
        // PINNED: the market has no independent basis source, so there is no
        // observation to smooth and no band to test. Handled ahead of the walk
        // because it is a property of the market, not of which legs answered.
        //
        // Falls through when FX is down, and that path is *not* the one the
        // other markets take: having no crypto leg is what makes a market
        // pinned, so a shut FX session leaves it with nothing live at all. The
        // others land on their crypto reference (`CryptoOnly`, healthy, §1 fm2)
        // while a pinned market lands on the static peg and runs `Degraded` —
        // tightened switches — for the whole FX-closed window. That suspends the
        // "never tighten a permanent condition" argument for `Unverified` above,
        // every weekend, and is accepted rather than overlooked: the alternative
        // it replaces was anchoring the weekend on the very reading this market
        // has no usable source for.
        if let (Some(pinned), Some(fx)) = (self.cfg.pinned_basis, fx) {
            return FairValue {
                basis: Some(pinned),
                // A pin is a constant, not an observation, so it has no age —
                // reporting one would invite the operator to read staleness into
                // a value that cannot go stale. And a pinned constant cannot
                // breach a band it was never measured against, so `basis_breach`
                // stays off: reporting one would be the false alarm this whole
                // path exists to remove.
                uncertain: fx.uncertain(self.cfg.fx_max_confidence_frac),
                usdc_breach,
                ..FairValue::of(Regime::FxPinned, Anchor::Fx, Some(fx.value * pinned))
            };
        }

        match (fx, crypto) {
            // NORMAL: both legs live — fair = fx × basis, basis = EMA(crypto/fx).
            (Some(fx), Some(crypto)) => {
                let observation = observed_basis(crypto.value, fx.value);
                // Band-test the **raw observation**, not only the smoothed
                // result. The outlier gate below can refuse a reading, and a
                // refused reading never moves the estimate — so testing only
                // the estimate would mean a source printing far outside the
                // sane band raised no peg event at all, precisely because it
                // was too wrong to fold. The breach signal must not be
                // conditional on the estimator having accepted the reading.
                let observed_breach = observation.is_some_and(|o| self.basis_out_of_band(o));
                let fold = observation.map(|o| self.basis.update(o, self.since_basis));
                let outlier = fold.is_none_or(Fold::rejected);
                if let Some(Fold::Folded(_)) = fold {
                    self.since_basis = Duration::ZERO;
                }

                // A refused reading leaves the estimate untouched and its age
                // running, so a run of them expires the basis exactly as an
                // absent leg would — which is the intended behaviour: a source
                // that has gone bad should not hold the basis alive.
                let Some(basis) = self.usable_basis() else {
                    return self.without_usable_basis(
                        legs.static_usd,
                        usdc_breach,
                        observed_breach,
                        outlier,
                    );
                };

                let regime = if outlier {
                    Regime::Degraded(Degrade::NoBasisLeg)
                } else {
                    Regime::Normal
                };
                FairValue {
                    basis: Some(basis),
                    basis_age: Some(self.since_basis),
                    uncertain: fx.uncertain(self.cfg.fx_max_confidence_frac),
                    basis_breach: observed_breach || self.basis_out_of_band(basis),
                    basis_outlier: outlier,
                    usdc_breach,
                    ..FairValue::of(regime, Anchor::Fx, Some(fx.value * basis))
                }
            }
            // FX live, basis leg down: anchor on FX with the last smoothed
            // basis, on thinner information — degraded. If that basis is
            // unusable (never seeded, or carried past its age bound) the engine
            // declines to invent one and falls back instead.
            (Some(fx), None) => {
                let Some(basis) = self.usable_basis() else {
                    return self.without_usable_basis(legs.static_usd, usdc_breach, false, false);
                };
                FairValue {
                    basis: Some(basis),
                    basis_age: Some(self.since_basis),
                    uncertain: fx.uncertain(self.cfg.fx_max_confidence_frac),
                    basis_breach: self.basis_out_of_band(basis),
                    usdc_breach,
                    ..FairValue::of(
                        Regime::Degraded(Degrade::NoBasisLeg),
                        Anchor::Fx,
                        Some(fx.value * basis),
                    )
                }
            }
            // No live FX: the crypto reference is the anchor. Structural on a
            // weekend (healthy, §1 fm2); an unexpected degrade otherwise (§4).
            // No FX ⇒ no observable basis.
            (None, Some(crypto)) => {
                // A weekend is structural whatever the FX leg did, so the
                // distinction below only refines an *unexpected* gap.
                let regime = if clock.weekend {
                    Regime::CryptoOnly
                } else if legs.fx.is_some_and(|r| r.young(stale) && !r.valid()) {
                    Regime::Degraded(Degrade::FxInvalid)
                } else {
                    Regime::Degraded(Degrade::FxStale)
                };
                FairValue {
                    usdc_breach,
                    ..FairValue::of(regime, Anchor::CryptoReference, Some(crypto.value))
                }
            }
            // Nothing live: the static peg if configured, else pause.
            (None, None) => self.fall_back(
                legs.static_usd,
                Degrade::StaticPeg,
                usdc_breach,
                false,
                false,
            ),
        }
    }

    /// The carried basis, if it is both seeded **and** within its age bound.
    ///
    /// The crate bounds the age of every input leg and, until this existed,
    /// nothing bounded the age of its own state — so `fair = fx × basis` could
    /// be quoted indefinitely off an estimate no longer connected to anything
    /// live. `since_basis` is the age directly: it accumulates every tick and
    /// resets only when an observation is actually folded.
    fn usable_basis(&self) -> Option<f64> {
        self.basis
            .value()
            .filter(|v| v.is_finite())
            .filter(|_| self.since_basis <= self.cfg.basis_max_age)
    }

    /// The composition for an FX-anchored market whose basis is unusable.
    fn without_usable_basis(
        &self,
        static_usd: f64,
        usdc_breach: bool,
        basis_breach: bool,
        basis_outlier: bool,
    ) -> FairValue {
        self.fall_back(
            static_usd,
            Degrade::BasisUnusable,
            usdc_breach,
            basis_breach,
            basis_outlier,
        )
    }

    /// The static peg if one is configured, else paused. The shared tail of
    /// every path that has no composable mid left.
    fn fall_back(
        &self,
        static_usd: f64,
        degrade: Degrade,
        usdc_breach: bool,
        basis_breach: bool,
        basis_outlier: bool,
    ) -> FairValue {
        let usable_peg = static_usd.is_finite() && static_usd > 0.0;
        let (regime, anchor) = if usable_peg {
            (Regime::Degraded(degrade), Anchor::Static)
        } else {
            (Regime::Paused, Anchor::None)
        };
        FairValue {
            basis_breach,
            basis_outlier,
            usdc_breach,
            ..FairValue::of(regime, anchor, usable_peg.then_some(static_usd))
        }
    }

    /// Whether a basis is outside this market's sane band (§4 basis-band
    /// breach).
    ///
    /// Public so a consumer banding a *raw first observation* — a wiring check,
    /// asking a different question of a different quantity than the engine's
    /// own test of the smoothed value — uses this market's configured band
    /// rather than re-deriving the comparison.
    pub fn basis_out_of_band(&self, basis: f64) -> bool {
        basis < self.cfg.basis_low || basis > self.cfg.basis_high
    }
}

/// The two-peg basis implied by one crypto reading and one FX reading:
/// `(token/USDC) ÷ (fiat/USD)`, in the units §1 defines. `None` when the anchor
/// is not a usable divisor or the result is not finite.
///
/// Exported so the consumer's first-basis wiring check bands the same quantity
/// the engine does, rather than re-deriving it — the duplicate it replaces had
/// already drifted into re-implementing the crate's leg gating alongside it.
pub fn observed_basis(crypto_usdc: f64, fx: f64) -> Option<f64> {
    if !fx.is_finite() || fx <= 0.0 || !crypto_usdc.is_finite() {
        return None;
    }
    let basis = crypto_usdc / fx;
    basis.is_finite().then_some(basis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn engine() -> FairValueEngine {
        FairValueEngine::new(FairValueConfig::default())
    }

    /// A reading fresh enough to pass the default 5-minute staleness bound.
    fn fresh(value: f64) -> Reading {
        Reading::new(value, secs(1))
    }

    /// An engine for a market with no independent basis source, pinned at 1.0.
    fn pinned_engine() -> FairValueEngine {
        FairValueEngine::new(FairValueConfig {
            pinned_basis: Some(1.0),
            ..FairValueConfig::default()
        })
    }

    #[test]
    fn pinned_market_anchors_on_fx_and_reports_unverified() {
        let mut e = pinned_engine();
        let legs = Legs {
            fx: Some(fresh(0.0573)),
            crypto_usdc: None,
            usdc_usd: Some(fresh(1.0)),
            static_usd: 0.0573,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::FxPinned);
        assert_eq!(r.anchor, Anchor::Fx);
        assert_eq!(r.health, Health::Unverified);
        assert_eq!(r.basis, Some(1.0));
        assert!((r.fair.unwrap() - 0.0573).abs() < 1e-12);
        // Unverified is not a degrade: the switches must not tighten forever.
        assert!(!r.degraded());
    }

    /// The regression this whole path exists to prevent: the old behavior fed a
    /// garbage index price into the basis, which sat outside the band on every
    /// tick and reported a standing BREACH. A pinned market must never breach.
    #[test]
    fn pinned_market_never_reports_a_basis_breach() {
        let mut e = pinned_engine();
        // A crypto reading roughly half the anchor — exactly the MXNe case that
        // breached. It must be ignored, not folded into the basis.
        let legs = Legs {
            fx: Some(fresh(0.0573)),
            crypto_usdc: Some(fresh(0.03064)),
            usdc_usd: Some(fresh(1.0)),
            static_usd: 0.0573,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::FxPinned);
        assert_eq!(r.basis, Some(1.0));
        assert!(!r.basis_breach);
        assert!((r.fair.unwrap() - 0.0573).abs() < 1e-12);
    }

    /// A pinned market is still subject to every non-basis guard: losing the FX
    /// anchor degrades it to the static peg exactly like any other market.
    #[test]
    fn pinned_market_without_fx_falls_to_the_static_peg() {
        let mut e = pinned_engine();
        let legs = Legs {
            fx: None,
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: 0.0573,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Degraded(Degrade::StaticPeg));
        assert_eq!(r.anchor, Anchor::Static);
        assert_eq!(r.health, Health::Degraded);
    }

    /// The USDC/USD common-mode guard is regime-independent, so it must still
    /// fire for a pinned market (§1 fm1).
    #[test]
    fn pinned_market_still_reports_a_usdc_breach() {
        let mut e = pinned_engine();
        let legs = Legs {
            fx: Some(fresh(0.0573)),
            crypto_usdc: None,
            usdc_usd: Some(fresh(0.80)),
            static_usd: 0.0573,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::FxPinned);
        assert!(r.usdc_breach);
    }

    #[test]
    fn normal_regime_composes_fx_times_basis() {
        // FX 1.14, crypto 1.14 → observed basis 1.0 → fair = 1.14 × 1.0.
        let mut e = engine();
        let legs = Legs {
            fx: Some(fresh(1.14)),
            crypto_usdc: Some(fresh(1.14)),
            usdc_usd: Some(fresh(1.0)),
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Normal);
        assert_eq!(r.anchor, Anchor::Fx);
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.basis, Some(1.0));
        assert!((r.fair.unwrap() - 1.14).abs() < 1e-12);
        assert!(!r.basis_breach && !r.usdc_breach);
    }

    #[test]
    fn basis_corrects_the_anchor() {
        // FX 1.10 but the token trades at 1.122 in USDC → basis 1.02 → the
        // anchor is corrected up to the market, not left at the raw FX.
        let mut e = engine();
        let legs = Legs {
            fx: Some(fresh(1.10)),
            crypto_usdc: Some(fresh(1.122)),
            usdc_usd: Some(fresh(1.0)),
            static_usd: 1.10,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert!((r.basis.unwrap() - 1.02).abs() < 1e-9);
        assert!((r.fair.unwrap() - 1.122).abs() < 1e-9);
    }

    #[test]
    fn does_not_collapse_usdc_to_one() {
        // When the crypto reference is genuinely USDC-denominated (Coinbase
        // <token>/USDC), a USDC premium shows up in that price and rides
        // through basis = crypto_usdc / fx into the mid — the model never
        // assumes USDC = USD. (The engine does not read the separate usdc_usd
        // leg into the mid; that leg drives only the common-mode guard.)
        let mut e = engine();
        // FX 1.0; the token trades at 0.98 USDC (USDC itself rich at 1.02 USD).
        // observed basis = 0.98 → fair = 0.98 USDC per token.
        let legs = Legs {
            fx: Some(fresh(1.0)),
            crypto_usdc: Some(fresh(0.98)),
            usdc_usd: Some(fresh(1.02)),
            static_usd: 1.0,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert!((r.fair.unwrap() - 0.98).abs() < 1e-9);
    }

    #[test]
    fn weekend_crypto_only_is_healthy() {
        // No FX (session closed) but the crypto reference is live → it is the
        // anchor, healthy, not degraded (§1 fm2). This is the demo path.
        let mut e = engine();
        let legs = Legs {
            fx: None,
            crypto_usdc: Some(fresh(1.14)),
            usdc_usd: None,
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::weekend());
        assert_eq!(r.regime, Regime::CryptoOnly);
        assert_eq!(r.anchor, Anchor::CryptoReference);
        assert_eq!(r.health, Health::Ok);
        assert_eq!(r.fair, Some(1.14));
        assert_eq!(r.basis, None);
    }

    #[test]
    fn fx_stale_outside_weekend_is_degraded() {
        // Same legs, but not a weekend → an unexpected FX outage → degraded.
        let mut e = engine();
        let legs = Legs {
            fx: None,
            crypto_usdc: Some(fresh(1.14)),
            usdc_usd: None,
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Degraded(Degrade::FxStale));
        assert_eq!(r.anchor, Anchor::CryptoReference);
        assert!(r.degraded());
        assert_eq!(r.fair, Some(1.14));
    }

    #[test]
    fn fx_live_without_basis_leg_uses_last_basis() {
        // Seed a basis in the normal regime, then drop the crypto leg: the mid
        // holds the last smoothed basis on the live FX, degraded.
        let mut e = engine();
        let seed = Legs {
            fx: Some(fresh(1.10)),
            crypto_usdc: Some(fresh(1.122)), // basis 1.02
            usdc_usd: None,
            static_usd: 1.10,
        };
        e.compose(seed, secs(5), ClockCtx::in_session());
        let no_basis = Legs {
            fx: Some(fresh(1.10)),
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: 1.10,
        };
        let r = e.compose(no_basis, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Degraded(Degrade::NoBasisLeg));
        assert!(r.degraded());
        assert!((r.fair.unwrap() - 1.10 * 1.02).abs() < 1e-9);
    }

    #[test]
    fn full_degrade_falls_to_static() {
        let mut e = engine();
        let legs = Legs {
            fx: None,
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Degraded(Degrade::StaticPeg));
        assert_eq!(r.anchor, Anchor::Static);
        assert_eq!(r.fair, Some(1.14));
    }

    #[test]
    fn pauses_only_without_a_static_peg() {
        let mut e = engine();
        let legs = Legs {
            fx: None,
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: 0.0,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Paused);
        assert_eq!(r.anchor, Anchor::None);
        assert_eq!(r.health, Health::Pause);
        assert!(r.fair.is_none());
    }

    #[test]
    fn stale_legs_drop_out() {
        // A stale FX + stale crypto with a live static → static peg.
        let mut e = engine();
        let legs = Legs {
            fx: Some(Reading::new(1.14, secs(600))),
            crypto_usdc: Some(Reading::new(1.14, secs(600))),
            usdc_usd: None,
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert_eq!(r.anchor, Anchor::Static);
    }

    #[test]
    fn usdc_common_mode_breach_flags_in_any_regime() {
        // A USDC/USD reading well outside the band raises the portfolio-wide
        // flag even in the normal regime, without blocking the mid.
        let mut e = engine();
        let legs = Legs {
            fx: Some(fresh(1.14)),
            crypto_usdc: Some(fresh(1.14)),
            usdc_usd: Some(fresh(0.90)), // depeg past the 0.97 floor
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert!(r.usdc_breach);
        assert!(r.fair.is_some());
    }

    #[test]
    fn basis_band_breach_flags_a_peg_event() {
        // Drive the smoothed basis past the placeholder 1.10 ceiling and hold
        // it there until the EMA crosses the band.
        let mut e = engine();
        let mut r = e.compose(
            Legs {
                fx: Some(fresh(1.0)),
                crypto_usdc: Some(fresh(1.30)), // observed basis 1.30
                usdc_usd: None,
                static_usd: 1.0,
            },
            secs(5),
            ClockCtx::in_session(),
        );
        for _ in 0..200 {
            r = e.compose(
                Legs {
                    fx: Some(fresh(1.0)),
                    crypto_usdc: Some(fresh(1.30)),
                    usdc_usd: None,
                    static_usd: 1.0,
                },
                secs(60),
                ClockCtx::in_session(),
            );
        }
        assert!(r.basis.unwrap() > 1.10);
        assert!(r.basis_breach);
    }

    #[test]
    fn uncertain_fx_quotes_but_flags() {
        // A fresh FX reading with a wide confidence band quotes, but raises the
        // fresh-but-uncertain flag (§1 fm6) — quote wider, don't halt.
        let mut e = engine();
        let legs = Legs {
            fx: Some(Reading::with_confidence(1.14, secs(1), 0.05)),
            crypto_usdc: Some(fresh(1.14)),
            usdc_usd: None,
            static_usd: 1.14,
        };
        let r = e.compose(legs, secs(5), ClockCtx::in_session());
        assert!(r.uncertain);
        assert_eq!(r.health, Health::Ok);
        assert!(r.fair.is_some());
    }

    /// Legs for the normal regime against a 1.0 anchor, so the observed basis
    /// equals `crypto` directly.
    fn normal(crypto: f64) -> Legs {
        Legs {
            fx: Some(fresh(1.0)),
            crypto_usdc: Some(fresh(crypto)),
            usdc_usd: None,
            static_usd: 1.0,
        }
    }

    #[test]
    fn a_gap_still_weights_the_returning_observation_more() {
        // The legitimate half of the re-seed design, kept: decay is driven by
        // time since the last *basis update*, not since the last compose, so a
        // gap weights the returning observation more than a per-tick alpha
        // would. Held inside the age bound and the jump gate so this measures
        // the decay and nothing else.
        let mut gapped = engine();
        let mut ticking = engine();
        gapped.compose(normal(1.0), secs(5), ClockCtx::in_session());
        ticking.compose(normal(1.0), secs(5), ClockCtx::in_session());

        // One long gap with no basis leg, well inside `basis_max_age`.
        gapped.compose(
            Legs {
                crypto_usdc: None,
                ..normal(1.0)
            },
            secs(30 * 60),
            ClockCtx::in_session(),
        );

        let g = gapped
            .compose(normal(1.04), secs(5), ClockCtx::in_session())
            .basis
            .unwrap();
        let t = ticking
            .compose(normal(1.04), secs(5), ClockCtx::in_session())
            .basis
            .unwrap();
        assert!(g > t, "the gapped engine ({g}) should weight more than {t}");
    }

    #[test]
    fn one_print_after_a_gap_no_longer_becomes_the_basis() {
        // The defect this issue exists to close. Previously the ~60h gap drove
        // the blend weight to 1, so the single returning observation landed the
        // estimate on its raw value — one print determining the whole
        // multiplicative correction, on the schedule of every session reopen.
        //
        // Now two independent guards stop it: the observation is a 10% move
        // against the running estimate, so the jump gate refuses it outright;
        // and the carried basis has aged past its bound, so there is no estimate
        // left to quote off either. The engine falls to the static peg rather
        // than inventing a basis.
        let mut e = engine();
        e.compose(normal(1.0), secs(5), ClockCtx::in_session());
        for _ in 0..10 {
            e.compose(
                Legs {
                    fx: None,
                    ..normal(1.0)
                },
                secs(6 * 3_600),
                ClockCtx::in_session(),
            );
        }
        let r = e.compose(normal(1.10), secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Degraded(Degrade::BasisUnusable));
        assert_eq!(r.anchor, Anchor::Static);
        assert_eq!(r.basis, None, "no basis may be reported, invented or stale");
        assert!(r.basis_outlier, "the refused observation must be surfaced");
    }

    #[test]
    fn a_carried_basis_expires_rather_than_quoting_forever() {
        // FX live, basis leg dead. The mid rides the last smoothed basis while
        // it is young, and stops when it is not — previously it rode it forever,
        // with a six-second-old basis and a five-day-old one producing
        // byte-identical output.
        let mut e = engine();
        e.compose(normal(1.02), secs(5), ClockCtx::in_session());
        let no_leg = Legs {
            crypto_usdc: None,
            ..normal(1.02)
        };

        let young = e.compose(no_leg, secs(30 * 60), ClockCtx::in_session());
        assert_eq!(young.regime, Regime::Degraded(Degrade::NoBasisLeg));
        assert_eq!(young.basis, Some(1.02));
        // The seeding tick folded, so the age restarts from zero there.
        assert_eq!(young.basis_age, Some(secs(30 * 60)));

        let expired = e.compose(no_leg, secs(60 * 60), ClockCtx::in_session());
        assert_eq!(expired.regime, Regime::Degraded(Degrade::BasisUnusable));
        assert_eq!(expired.anchor, Anchor::Static);
        assert_eq!(expired.basis, None);
        assert_eq!(expired.basis_age, None);
    }

    #[test]
    fn a_never_seeded_basis_is_not_reported_as_parity() {
        // The engine used to substitute 1.0 for an unobserved basis, which is
        // indistinguishable in the output from having measured the basis and
        // found it at par. An unseeded engine must report no basis at all.
        let mut e = engine();
        let r = e.compose(
            Legs {
                crypto_usdc: None,
                ..normal(1.0)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.regime, Regime::Degraded(Degrade::BasisUnusable));
        assert_eq!(r.basis, None);
        assert_eq!(r.fair, Some(1.0), "the static peg carries the mid");
    }

    #[test]
    fn an_unseeded_engine_pauses_without_a_static_peg() {
        // Same path, but with nothing to fall back to.
        let mut e = engine();
        let r = e.compose(
            Legs {
                crypto_usdc: None,
                static_usd: 0.0,
                ..normal(1.0)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.regime, Regime::Paused);
        assert!(r.fair.is_none());
    }

    #[test]
    fn an_out_of_band_print_still_raises_a_peg_event_when_refused() {
        // The interaction that would otherwise be a silent regression: the
        // outlier gate refuses a wild reading, and a refused reading never moves
        // the estimate — so banding only the estimate would mean the wildest
        // prints raised no alarm at all, precisely because they were too wrong
        // to fold. The raw observation is banded independently.
        let mut e = engine();
        e.compose(normal(1.0), secs(5), ClockCtx::in_session());
        let r = e.compose(normal(0.52), secs(30), ClockCtx::in_session());
        assert!(r.basis_outlier, "0.52 against 1.0 is far outside the gate");
        assert!(
            r.basis_breach,
            "and it is outside the sane band → peg event"
        );
        assert_eq!(r.basis, Some(1.0), "the estimate is untouched");
        assert!(r.degraded());
    }

    #[test]
    fn a_refused_print_does_not_reset_the_basis_age() {
        // A sick source must not hold the basis alive by answering with
        // garbage; only a folded observation counts as an observation.
        let mut e = engine();
        e.compose(normal(1.0), secs(5), ClockCtx::in_session());
        let r = e.compose(normal(0.52), secs(30), ClockCtx::in_session());
        assert_eq!(r.basis_age, Some(secs(30)));
        let r = e.compose(normal(0.52), secs(30), ClockCtx::in_session());
        assert_eq!(r.basis_age, Some(secs(60)), "age keeps running");
    }

    #[test]
    fn pinned_market_with_a_live_crypto_leg_and_no_fx_never_prices_off_crypto() {
        // The fallthrough the engine previously argued was unreachable. It was
        // right that no configuration reaches it today — a pinned market carries
        // no basis-source ids — but the guarantee lived in the consumer's test
        // suite rather than in this crate. Now the pin drops the crypto leg
        // unconditionally, so the case is closed here.
        let mut e = pinned_engine();
        let r = e.compose(
            Legs {
                fx: None,
                crypto_usdc: Some(fresh(0.03064)),
                usdc_usd: Some(fresh(1.0)),
                static_usd: 0.0573,
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.anchor, Anchor::Static);
        assert_eq!(
            r.fair,
            Some(0.0573),
            "must not price off the crypto reading the pin declares unusable"
        );
    }

    #[test]
    fn pinned_market_on_a_weekend_falls_to_the_static_peg() {
        // The weekend counterpart of the case above: other markets flip to
        // their crypto reference, but a pinned market has none to flip to.
        let mut e = pinned_engine();
        let r = e.compose(
            Legs {
                fx: None,
                crypto_usdc: Some(fresh(0.03064)),
                usdc_usd: None,
                static_usd: 0.0573,
            },
            secs(5),
            ClockCtx::weekend(),
        );
        assert_eq!(r.anchor, Anchor::Static);
        assert_eq!(r.fair, Some(0.0573));
    }

    #[test]
    fn a_pinned_basis_reports_no_age() {
        let mut e = pinned_engine();
        let r = e.compose(
            Legs {
                fx: Some(fresh(0.0573)),
                crypto_usdc: None,
                usdc_usd: None,
                static_usd: 0.0573,
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.basis, Some(1.0));
        assert_eq!(r.basis_age, None, "a constant has no observation age");
    }

    /// A consumer's startup wiring check spends one shot on the first basis it
    /// can observe, so "observable" has to mean both legs actually present and
    /// fresh — the feed sources warm asynchronously, and an early partial leg
    /// set must not count as checked.
    #[test]
    fn a_basis_is_observable_only_when_both_legs_are_live_and_fresh() {
        let stale = secs(300);
        let base = Legs {
            fx: Some(fresh(0.0573)),
            crypto_usdc: Some(fresh(0.0573)),
            usdc_usd: Some(fresh(1.0)),
            static_usd: 0.0573,
        };
        assert_eq!(base.observed_basis(stale), Some(1.0));

        // Either leg missing — nothing to observe yet.
        assert_eq!(Legs { fx: None, ..base }.observed_basis(stale), None);
        assert_eq!(
            Legs {
                crypto_usdc: None,
                ..base
            }
            .observed_basis(stale),
            None
        );
        // A leg present but stale is not a reading.
        assert_eq!(
            Legs {
                fx: Some(Reading::new(0.0573, secs(600))),
                ..base
            }
            .observed_basis(stale),
            None
        );
        // A non-positive anchor would divide by zero.
        assert_eq!(
            Legs {
                fx: Some(fresh(0.0)),
                ..base
            }
            .observed_basis(stale),
            None
        );
        // The thin-market shape that motivated this work: a basis near 0.53.
        let observed = Legs {
            crypto_usdc: Some(fresh(0.03064)),
            ..base
        }
        .observed_basis(stale)
        .unwrap();
        assert!((observed - 0.5347).abs() < 1e-3, "observed {observed}");
    }

    #[test]
    fn observed_basis_is_the_documented_two_peg_quantity() {
        // (token/USDC) ÷ (fiat/USD). The division is embedded in the leg units,
        // which is why the USDC premium is deliberately not collapsed away.
        assert_eq!(observed_basis(1.141, 1.14), Some(1.141 / 1.14));
        // A non-positive or non-finite anchor has no usable reciprocal.
        assert_eq!(observed_basis(1.141, 0.0), None);
        assert_eq!(observed_basis(1.141, -1.0), None);
        assert_eq!(observed_basis(1.141, f64::NAN), None);
        assert_eq!(observed_basis(f64::NAN, 1.14), None);
    }

    #[test]
    fn a_garbage_fx_print_is_not_reported_as_a_stale_feed() {
        // Same composition either way — both are unusable — but the operator is
        // told which failure it is. A dead poller and a live feed publishing
        // garbage previously carried the identical label.
        let mut e = engine();
        let r = e.compose(
            Legs {
                fx: Some(fresh(f64::NAN)),
                ..normal(1.14)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.regime, Regime::Degraded(Degrade::FxInvalid));

        let r = e.compose(
            Legs {
                fx: Some(Reading::new(1.14, secs(600))),
                ..normal(1.14)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.regime, Regime::Degraded(Degrade::FxStale));

        let r = e.compose(
            Legs {
                fx: None,
                ..normal(1.14)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(
            r.regime,
            Regime::Degraded(Degrade::FxStale),
            "absent is not invalid"
        );
    }

    #[test]
    fn a_weekend_stays_structural_whatever_the_fx_leg_did() {
        // The refinement above must not turn a closed session into a fault.
        let mut e = engine();
        let r = e.compose(
            Legs {
                fx: Some(fresh(f64::NAN)),
                ..normal(1.14)
            },
            secs(5),
            ClockCtx::weekend(),
        );
        assert_eq!(r.regime, Regime::CryptoOnly);
        assert_eq!(r.health, Health::Ok);
    }

    #[test]
    fn health_is_a_total_function_of_regime() {
        // Every variant, so a regime added later cannot quietly inherit a
        // health that does not follow from it.
        assert_eq!(Regime::Normal.health(), Health::Ok);
        assert_eq!(Regime::CryptoOnly.health(), Health::Ok);
        assert_eq!(Regime::FxPinned.health(), Health::Unverified);
        assert_eq!(Regime::Paused.health(), Health::Pause);
        for d in [
            Degrade::FxStale,
            Degrade::FxInvalid,
            Degrade::NoBasisLeg,
            Degrade::BasisUnusable,
            Degrade::StaticPeg,
        ] {
            assert_eq!(Regime::Degraded(d).health(), Health::Degraded, "{d:?}");
        }
    }

    #[test]
    fn every_composition_reports_the_health_its_regime_implies() {
        // The invariant the derivation exists to hold, checked through
        // `compose` rather than on the mapping alone: whatever arm ran, the two
        // fields agree.
        let mut e = engine();
        let cases = [
            (normal(1.02), ClockCtx::in_session()),
            (
                Legs {
                    crypto_usdc: None,
                    ..normal(1.02)
                },
                ClockCtx::in_session(),
            ),
            (
                Legs {
                    fx: None,
                    ..normal(1.02)
                },
                ClockCtx::weekend(),
            ),
            (
                Legs {
                    fx: None,
                    ..normal(1.02)
                },
                ClockCtx::in_session(),
            ),
            (
                Legs {
                    fx: None,
                    crypto_usdc: None,
                    ..normal(1.02)
                },
                ClockCtx::in_session(),
            ),
            (
                Legs {
                    fx: None,
                    crypto_usdc: None,
                    static_usd: 0.0,
                    ..normal(1.02)
                },
                ClockCtx::in_session(),
            ),
        ];
        for (legs, clock) in cases {
            let r = e.compose(legs, secs(5), clock);
            assert_eq!(r.health, r.regime.health(), "regime {:?}", r.regime);
        }

        // And the pinned arm, which is the one regime a normal engine cannot
        // reach.
        let r = pinned_engine().compose(
            Legs {
                crypto_usdc: None,
                ..normal(1.0)
            },
            secs(5),
            ClockCtx::in_session(),
        );
        assert_eq!(r.regime, Regime::FxPinned);
        assert_eq!(r.health, r.regime.health());
    }

    #[test]
    fn a_normal_tick_reports_a_zero_basis_age() {
        let mut e = engine();
        let r = e.compose(normal(1.02), secs(5), ClockCtx::in_session());
        assert_eq!(r.regime, Regime::Normal);
        assert_eq!(r.basis_age, Some(Duration::ZERO));
        assert!(!r.basis_outlier);
    }
}
