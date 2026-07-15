//! Off-chain FX-anchor × basis fair-value engine (docs/market-making-mvp.md
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
//! fair-value taker and the pricing-survey harness are separate follow-ups that
//! exercise the same code.
//!
//! # Calibration
//!
//! Almost every constant the engine reads is **TBD — set by the pricing-model
//! survey**. The [`FairValueConfig`] defaults are marked, demo-safe
//! placeholders, not calibrated values; recalibration is a data edit to that
//! one surface. See its module docs.
//!
//! # Usage
//!
//! One [`FairValueEngine`] per market (each carries its own basis history).
//! Each tick, build [`Legs`] from the live feeds and call
//! [`FairValueEngine::compose`]:
//!
//! ```
//! use std::time::Duration;
//! use dropset_fair_value::{FairValueConfig, FairValueEngine, Legs, Reading};
//!
//! let mut engine = FairValueEngine::new(FairValueConfig::default());
//! let legs = Legs {
//!     fx: Some(Reading::new(1.14, Duration::from_secs(1))),       // EUR/USD
//!     crypto_usdc: Some(Reading::new(1.141, Duration::from_secs(1))), // EURC/USDC
//!     usdc_usd: Some(Reading::new(1.0, Duration::from_secs(1))),
//!     static_usd: 1.14,
//! };
//! let fair = engine.compose(legs, Duration::from_secs(5), false);
//! assert!(fair.fair.is_some());
//! ```

mod basis;
mod config;
mod engine;
mod reading;

pub use config::FairValueConfig;
pub use engine::{Anchor, Degrade, FairValue, FairValueEngine, Health, Legs, Regime};
pub use reading::Reading;
