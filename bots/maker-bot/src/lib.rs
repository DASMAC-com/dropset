//! `dropset-maker-bot` — the localnet FX-stablecoin market-maker.
//!
//! A supervisor over many `<token>/USDC` markets ([`config::MARKETS`]) quoting
//! on the eCLOB per `docs/market-making.md`. One shared leader quotes every
//! market; each cycle the bot refreshes a batched, tiered price feed
//! (CoinGecko → CoinMarketCap → ECB/Frankfurter FX-rate → static), composes a
//! per-market fair mid, and drives the program's relative-quoting hot path
//! (`set_reference_price`, with an inventory skew) and cold path
//! (`set_liquidity_profile`), under the spec's inventory / peg / staleness
//! kill switches.
//!
//! The crate splits into the dropset-alpha shape:
//!
//! - [`config`] — the spec's knobs, with defaults encoding the MVP verbatim.
//! - [`model`] — the pure quoting logic (feeds, fair mid, ladder, skew,
//!   triggers, kill switches), deterministic and unit tested.
//!
//! The [`context`], [`chain`], and [`tasks`] modules layer the runtime state,
//! on-chain I/O, and the 5-second tick loop on top of this core, and
//! [`quote_state`] persists the one fact that has to outlive the process — when
//! each market's book was last correctly priced, which is what makes stale-quote
//! invalidation ([`model::invalidate`]) possible across a restart.

pub mod chain;
pub mod config;
pub mod context;
pub mod fills;
pub mod model;
pub mod quote_state;
pub mod tasks;
