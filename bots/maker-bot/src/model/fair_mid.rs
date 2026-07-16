//! Reference-price composition (§1) — the maker's adapter onto the shared
//! [`dropset_fair_value`] engine.
//!
//! The engine composes `fair = fx × basis` from three legs (§1); this module
//! only maps the bot's tiered feed cache onto those legs and re-exports the
//! engine's result types. The stateful engine itself (its per-market basis EMA)
//! lives on each market's [`crate::context::Context`], so the composition is
//! `ctx.engine.compose(legs, dt, weekend)`, not a free function.
//!
//! ## Legs, and how the bot's feeds map onto them
//!
//! | Engine leg    | Meaning              | Bot source (this PR)                    |
//! | ------------- | -------------------- | --------------------------------------- |
//! | `fx`          | USD per fiat unit    | ECB/Frankfurter USD/`<ccy>`             |
//! | `crypto_usdc` | USDC per token       | CoinGecko / CoinMarketCap token-USD     |
//! | `usdc_usd`    | USD per USDC         | CoinGecko `usd-coin`                    |
//! | `static_usd`  | last-resort peg      | [`crate::config::MarketConfig::static_usd`] |
//!
//! The crypto/USD tier (CoinGecko / CMC), which the *old* cascade used as the
//! primary mid, is **demoted** here to the basis leg — a fallback source, never
//! the anchor, for the reflexivity reason in §1. The spec's designated
//! streaming primaries (Pyth Hermes / OANDA for the anchor; Coinbase
//! `<token>/USDC`, Binance `EUR/USDT` for the basis; Circle redemption for
//! peg-truth) are a separate follow-up; until they land, Frankfurter serves as
//! the FX anchor — which is exactly the anchor's designated *fallback* tier, so
//! the two-peg model runs live on real data today.

use dropset_fair_value::{Legs, Reading};

pub use dropset_fair_value::FairValue;

/// Build the engine's [`Legs`] for one market from the bot's cached readings.
/// Each `Option` is `None` when that source didn't answer this tick; the engine
/// drops any that are stale and selects the regime from what's live.
pub fn build_legs(
    fx: Option<Reading>,
    crypto_usdc: Option<Reading>,
    usdc_usd: Option<Reading>,
    static_usd: f64,
) -> Legs {
    Legs {
        fx,
        crypto_usdc,
        usdc_usd,
        static_usd,
    }
}
