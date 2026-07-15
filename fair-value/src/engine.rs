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
//!   only EUR/USD discovery and becomes the anchor — healthy, not degraded.
//!   This is also the **localnet demo path**: the demo wires no FX anchor, so
//!   it runs here on the crypto reference (today's CoinGecko mid), unchanged.
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

use crate::basis::BasisEma;
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
    /// A degrade — see [`Degrade`] for which.
    Degraded(Degrade),
    /// No usable leg at all.
    Paused,
}

/// Why the engine is degraded (§1 "degraded and halt conditions", §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Degrade {
    /// FX anchor stale outside the weekend regime — the crypto reference
    /// carries the mid, but this is an unexpected gap, so run degraded (§4).
    FxStale,
    /// FX anchor live but the crypto basis leg is down — anchor on FX with the
    /// last smoothed basis (or 1.0 if none yet), on thinner information.
    NoBasisLeg,
    /// Every live leg down to the static peg — the deepest degraded case.
    StaticPeg,
}

/// Health gate for the quoting hot path — the axis the kill-switch policy reads
/// (§4). `Degraded` tightens the switches by 50%; `Pause` stops quoting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// Quote normally.
    Ok,
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
    /// Whether the kill-switch policy should run tightened (§4). True in every
    /// degraded regime; false when healthy or paused.
    pub fn degraded(&self) -> bool {
        self.health == Health::Degraded
    }
}

/// Per-market fair-value engine: the calibration constants plus the stateful
/// basis EMA. One instance per market (each carries its own basis history).
#[derive(Clone, Copy, Debug)]
pub struct FairValueEngine {
    cfg: FairValueConfig,
    basis: BasisEma,
}

impl FairValueEngine {
    /// Build an engine for one market from its calibration config.
    pub fn new(cfg: FairValueConfig) -> Self {
        Self {
            basis: BasisEma::new(cfg.basis_half_life),
            cfg,
        }
    }

    /// The current smoothed basis (for logging / inspection), or `None` before
    /// the first FX-anchored observation.
    pub fn basis(&self) -> Option<f64> {
        self.basis.value()
    }

    /// Compose the fair value for this market from its live `legs`. `dt` is the
    /// elapsed time since the previous `compose` (feeds the basis EMA decay);
    /// `weekend` marks the FX-closed session window (§1 fm2), inside which
    /// FX-stale is the normal crypto-only state rather than a degrade.
    pub fn compose(&mut self, legs: Legs, dt: Duration, weekend: bool) -> FairValue {
        let stale = self.cfg.leg_stale;
        let fx = legs.fx.filter(|r| r.fresh(stale));
        let crypto = legs.crypto_usdc.filter(|r| r.fresh(stale));
        let usdc = legs.usdc_usd.filter(|r| r.fresh(stale));

        // The USDC/USD common-mode guard is regime-independent: a depeg moves
        // every market's basis at once, so it is evaluated wherever a live
        // USDC/USD reading exists (§1 fm1).
        let usdc_breach =
            usdc.is_some_and(|u| u.value < self.cfg.usdc_low || u.value > self.cfg.usdc_high);

        match (fx, crypto) {
            // NORMAL: both legs live — fair = fx × basis, basis = EMA(crypto/fx).
            (Some(fx), Some(crypto)) => {
                let basis_obs = crypto.value / fx.value;
                let basis = self.basis.update(basis_obs, dt);
                let fair = fx.value * basis;
                FairValue {
                    fair: Some(fair),
                    anchor: Anchor::Fx,
                    regime: Regime::Normal,
                    basis: Some(basis),
                    health: Health::Ok,
                    uncertain: fx.uncertain(self.cfg.fx_max_confidence_frac),
                    basis_breach: self.basis_out_of_band(basis),
                    usdc_breach,
                }
            }
            // FX live, basis leg down: anchor on FX with the last smoothed basis
            // (or 1.0 until one exists), on thinner information — degraded.
            (Some(fx), None) => {
                let basis = self.basis.value().unwrap_or(1.0);
                let fair = fx.value * basis;
                FairValue {
                    fair: Some(fair),
                    anchor: Anchor::Fx,
                    regime: Regime::Degraded(Degrade::NoBasisLeg),
                    basis: Some(basis),
                    health: Health::Degraded,
                    uncertain: fx.uncertain(self.cfg.fx_max_confidence_frac),
                    basis_breach: self.basis_out_of_band(basis),
                    usdc_breach,
                }
            }
            // No live FX: the crypto reference is the anchor. Structural on a
            // weekend (healthy, §1 fm2); an unexpected degrade otherwise (§4).
            // This is the localnet demo path. No FX ⇒ no observable basis.
            (None, Some(crypto)) => {
                let (regime, health) = if weekend {
                    (Regime::CryptoOnly, Health::Ok)
                } else {
                    (Regime::Degraded(Degrade::FxStale), Health::Degraded)
                };
                FairValue {
                    fair: Some(crypto.value),
                    anchor: Anchor::CryptoReference,
                    regime,
                    basis: None,
                    health,
                    uncertain: false,
                    basis_breach: false,
                    usdc_breach,
                }
            }
            // Nothing live: the static peg if configured, else pause.
            (None, None) => {
                if legs.static_usd.is_finite() && legs.static_usd > 0.0 {
                    FairValue {
                        fair: Some(legs.static_usd),
                        anchor: Anchor::Static,
                        regime: Regime::Degraded(Degrade::StaticPeg),
                        basis: None,
                        health: Health::Degraded,
                        uncertain: false,
                        basis_breach: false,
                        usdc_breach,
                    }
                } else {
                    FairValue {
                        fair: None,
                        anchor: Anchor::None,
                        regime: Regime::Paused,
                        basis: None,
                        health: Health::Pause,
                        uncertain: false,
                        basis_breach: false,
                        usdc_breach,
                    }
                }
            }
        }
    }

    /// Whether a smoothed basis is outside its sane band (§4 basis-band breach).
    fn basis_out_of_band(&self, basis: f64) -> bool {
        basis < self.cfg.basis_low || basis > self.cfg.basis_high
    }
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
        let r = e.compose(legs, secs(5), false);
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
        let r = e.compose(legs, secs(5), false);
        assert!((r.basis.unwrap() - 1.02).abs() < 1e-9);
        assert!((r.fair.unwrap() - 1.122).abs() < 1e-9);
    }

    #[test]
    fn does_not_collapse_usdc_to_one() {
        // The basis is observed from crypto/fx, which folds in the USDC leg —
        // so a USDC premium is reflected in the mid, never assumed away.
        let mut e = engine();
        // FX 1.0; token trades at 0.98 USDC because USDC itself trades at 1.02
        // USD. observed basis = 0.98 → fair = 0.98 (in USDC per token).
        let legs = Legs {
            fx: Some(fresh(1.0)),
            crypto_usdc: Some(fresh(0.98)),
            usdc_usd: Some(fresh(1.02)),
            static_usd: 1.0,
        };
        let r = e.compose(legs, secs(5), false);
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
        let r = e.compose(legs, secs(5), true);
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
        let r = e.compose(legs, secs(5), false);
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
        e.compose(seed, secs(5), false);
        let no_basis = Legs {
            fx: Some(fresh(1.10)),
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: 1.10,
        };
        let r = e.compose(no_basis, secs(5), false);
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
        let r = e.compose(legs, secs(5), false);
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
        let r = e.compose(legs, secs(5), false);
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
        let r = e.compose(legs, secs(5), false);
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
        let r = e.compose(legs, secs(5), false);
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
            false,
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
                false,
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
        let r = e.compose(legs, secs(5), false);
        assert!(r.uncertain);
        assert_eq!(r.health, Health::Ok);
        assert!(r.fair.is_some());
    }
}
