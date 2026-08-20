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
//! selects them, it does not implement them. Each leg offers **every source
//! that answered** as a candidate set, and the engine resolves them by
//! consensus ([`crate::tasks`] does the collecting):
//!
//! | Engine leg    | Meaning           | Sources offered                                    |
//! | ------------- | ----------------- | -------------------------------------------------- |
//! | `fx`          | USD per fiat unit | Pyth Hermes (trusted), ECB/Frankfurter              |
//! | `crypto_usdc` | USDC per token    | Coinbase `/USDC`, Kraken `/USD`, CoinGecko, CMC     |
//! | `usdc_usd`    | USD per USDC      | Kraken `USDCUSD`, CoinGecko `usd-coin`             |
//! | `static_usd`  | last-resort peg   | [`crate::config::MarketConfig::static_usd`]        |
//!
//! What that table is load-bearing about:
//!
//! - **Listing order is not priority any more.** It used to be: the first tier
//!   that answered won outright, so a single bad source *was* the leg. Order now
//!   only decides which sources survive a set larger than the engine will hold,
//!   and nothing else.
//! - **The crypto/USD index sources are still the weakest input**, for the
//!   reflexivity reason in §1 fm5 — but only EURC is listed on Coinbase or
//!   Kraken, so for the other index-priced markets they are the *only* input.
//!   Those markets now compose as `Regime::Uncorroborated` and report
//!   `Health::Unverified`: still quoted, no longer described as corroborated.
//! - **One market has no source at all.** MXNe reaches none of them (see
//!   [`crate::config::MARKETS`]), so it composes on its FX anchor with a pinned
//!   basis. The engine drops its whole crypto candidate set unconditionally, so
//!   a stray reading for it can never price it.
//! - **Pyth is the one source designated believable alone**, because it
//!   publishes a confidence half-width (without one the fresh-but-uncertain
//!   regime, §1 fm6, is unobservable) and is aged from the publisher's clock.
//!   That designation is also what lets it stand when it and the daily ECB
//!   reference drift apart — a disagreement the leg still reports.
//! - **Staleness is the engine's rule, applied once.** The collection below no
//!   longer pre-filters on freshness: a stale candidate simply does not count
//!   toward its leg's consensus, so it cannot mask a live one either.
//! - **…except while the FX session is shut.** Frankfurter is aged from
//!   *receipt*, so it reads fresh all weekend off a Friday close. Offering it
//!   then would hold the engine in the Normal regime on a closed market, which
//!   is the "fall back to a stale peg" behavior §1 fm2 rejects — so it is
//!   withheld on weekends and the crypto reference anchors.
//!
//! One unit conversion happens during collection rather than in the engine:
//! Coinbase quotes `<token>/USDC` directly, but Kraken quotes `<token>/USD`, so
//! a Kraken candidate is divided by the peg leg's own consensus. The peg guard
//! only *alarms* at a deviation — it does not correct one — and leaving it
//! uncorrected would make the Kraken and Coinbase candidates disagree by the
//! width of the peg, turning a unit mismatch into a false dispersion flag.
//!
//! The remaining spec-named sources are not wired, and each for a reason
//! established by probing it: **Binance** answers `HTTP 451` from the deploy
//! region (and Binance.US lists no EUR pair at all), and **Circle** publishes
//! no keyless redemption-rate endpoint — Kraken's `USDC/USD` market print
//! stands in for peg truth until credentials exist. (Kraken lists `EURC/EUR`
//! too, the closer issuer-rate proxy, but nothing subscribes to it yet.)
//! OANDA is the same story as Circle.

use dropset_fair_value::{Candidates, Legs};

pub use dropset_fair_value::FairValue;

/// Build the engine's [`Legs`] for one market from the bot's cached readings.
/// Each leg carries every source that answered this tick; the engine drops the
/// stale and invalid ones, resolves the rest by consensus, and selects the
/// regime from what survives.
pub fn build_legs(
    fx: Candidates,
    crypto_usdc: Candidates,
    usdc_usd: Candidates,
    static_usd: f64,
) -> Legs {
    Legs {
        fx,
        crypto_usdc,
        usdc_usd,
        static_usd,
    }
}
