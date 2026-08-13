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
//! The sources are the shared `dropset_feeds::venues` adapters — the bot
//! selects and tiers them, it does not implement them. Each leg walks its tiers
//! in order and takes the first that answers ([`crate::tasks`] does the walk):
//!
//! | Engine leg    | Meaning           | Primary            | Then                 | Last resort   |
//! | ------------- | ----------------- | ------------------ | -------------------- | ------------- |
//! | `fx`          | USD per fiat unit | Pyth Hermes        | ECB/Frankfurter      | —             |
//! | `crypto_usdc` | USDC per token    | Coinbase `/USDC`   | Kraken `/USD`        | CoinGecko/CMC |
//! | `usdc_usd`    | USD per USDC      | Kraken `USDCUSD`   | CoinGecko `usd-coin` | —             |
//! | `static_usd`  | last-resort peg   | [`crate::config::MarketConfig::static_usd`] | | |
//!
//! Two things that table is load-bearing about:
//!
//! - **The crypto/USD index tier is demoted, not retired.** CoinGecko / CMC was
//!   the *old* cascade's primary mid and is a fallback here, for the
//!   reflexivity reason in §1 fm5 — but only EURC is listed on Coinbase or
//!   Kraken, so for the other six markets that fallback *is* the basis leg.
//! - **Pyth earns the anchor by publishing a confidence half-width**, which
//!   Frankfurter's daily ECB reference does not. Without one the
//!   fresh-but-uncertain regime (§1 fm6) is unobservable, so on the Frankfurter
//!   tier the engine can only ever see the anchor as fresh or stale.
//!
//! The remaining spec-named sources are not wired, and each for a reason
//! established by probing it: **Binance** answers `HTTP 451` from the deploy
//! region (and Binance.US lists no EUR pair at all), and **Circle** publishes
//! no keyless redemption-rate endpoint — Kraken's `USDC/USD` and `EURC/EUR`
//! market prints stand in for peg truth until credentials exist. OANDA is the
//! same story as Circle.

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
