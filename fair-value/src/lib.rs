//! Off-chain FX-anchor × basis fair-value engine (docs/market-making.md
//! §1).
//!
//! Fair value is a **fast, deep, exogenous FX driver corrected by a slow, thin
//! stablecoin basis**:
//!
//! ```text
//! fair  = fx_rate × basis
//! basis = EMA of (token/fiat ÷ USDC/USD)      # a multiplicative correction near 1
//! ```
//!
//! This inverts the old cascade that made the token's crypto/USD price the
//! primary mid: that feed is laggy and *reflexive* (it is derived in part from
//! the very prints we race), so anchoring on it makes the bot lag exactly when
//! the edge appears. Here the exogenous FX cross is the anchor and the crypto
//! venues only supply the slow basis correction.
//!
//! # What lives here, and what doesn't
//!
//! This crate is the **shared, `std`-only, unit-tested model**: the reading
//! freshness rules, the stateful basis EMA, the composition and its regimes,
//! and the guard signals. It is deliberately **not** in `sdk/math-core` — that
//! is the audit-pinned, integer, on-chain consensus math, and this is
//! off-chain `f64` network-fed strategy code; keeping them apart keeps the
//! audit surface minimal.
//!
//! Feed **transport** (the HTTP polling of Pyth Hermes / OANDA / Coinbase /
//! Circle / CoinGecko) is *not* here — each consumer owns its own thin
//! transport and hands the engine [`Reading`]s. The valuable shared thing is
//! the model, not the I/O. The maker bot consumes the engine today; the
//! fair-value taker is a separate follow-up that exercises the same code.
//!
//! # Calibration
//!
//! Almost every constant the engine reads is **TBD — set by the analytics over
//! collected market-data history** (`docs/data-feeds.md` §11). The
//! [`FairValueConfig`] defaults are marked, demo-safe placeholders, not
//! calibrated values; recalibration is a data edit to that one surface. See
//! its module docs.
//!
//! # Usage
//!
//! One [`FairValueEngine`] per market (each carries its own basis history).
//! Each tick, offer every source that answered for each leg and call
//! [`FairValueEngine::compose`]. The engine resolves each leg by consensus —
//! median across three or more, agree-or-degrade across two, an explicit
//! single-source state for one — so the caller collects candidates rather than
//! picking a winner:
//!
//! ```
//! use std::time::Duration;
//! use dropset_fair_value::{Candidates, ClockCtx, FairValueConfig, FairValueEngine, Legs, Reading};
//!
//! let age = Duration::from_secs(1);
//! let mut engine = FairValueEngine::new(FairValueConfig::default());
//! let legs = Legs {
//!     // A first-party oracle may stand alone; everything else needs company.
//!     fx: Candidates::none()
//!         .push_trusted("pyth-hermes", Some(Reading::new(1.14, age))),   // EUR/USD
//!     crypto_usdc: Candidates::none()
//!         .push("coinbase", Some(Reading::new(1.141, age)))              // EURC/USDC
//!         .push("kraken", Some(Reading::new(1.142, age)))
//!         .push("coingecko", Some(Reading::new(1.140, age))),
//!     usdc_usd: Candidates::none().push("kraken", Some(Reading::new(1.0, age))),
//!     static_usd: 1.14,
//! };
//! let fair = engine.compose(legs, Duration::from_secs(5), ClockCtx::in_session());
//! assert!(fair.fair.is_some());
//! // Three sources agreed, so the basis leg is corroborated and none is an outlier.
//! assert_eq!(fair.crypto_leg.n, 3);
//! assert_eq!(fair.crypto_leg.outlier, None);
//! ```

mod basis;
mod config;
mod consensus;
mod engine;
mod reading;

pub use config::{ConfigError, FairValueConfig};
pub use consensus::{
    Candidate, Candidates, Consensus, ConsensusState, Contributor, Contributors, MAX_CANDIDATES,
};
pub use engine::{
    observed_basis, Anchor, ClockCtx, Degrade, FairValue, FairValueEngine, Health, LegReport, Legs,
    Regime,
};
pub use reading::Reading;
