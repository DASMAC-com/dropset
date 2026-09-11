// cspell:word unmigrated
//! `dropset-maker-bot` — the localnet FX-stablecoin market-maker.
//!
//! A supervisor over many `<token>/USDC` markets ([`config::MARKETS`]) quoting
//! on the eCLOB per `docs/market-making.md`. One shared leader quotes every
//! market; each cycle the bot refreshes a batched price feed (the Frankfurter
//! FX anchor, the CoinGecko / CoinMarketCap crypto basis leg, and a static peg
//! of last resort), composes a
//! per-market fair mid, and drives the program's relative-quoting hot path
//! (`set_reference_price`, with an inventory skew) and cold path
//! (`set_liquidity_profile`), under the spec's inventory / peg / staleness
//! kill switches.
//!
//! The crate splits into the dropset-alpha shape:
//!
//! - [`config`] — the spec's knobs, with defaults encoding it verbatim.
//! - [`model`] — the pure quoting logic (feeds, fair mid, ladder, skew,
//!   triggers, kill switches), deterministic and unit tested.
//!
//! The [`context`], [`chain`], and [`tasks`] modules layer the runtime state,
//! on-chain I/O, and the 5-second tick loop on top of this core, and
//! [`quote_state`] persists the one fact that has to outlive the process — when
//! each market's book was last correctly priced, which is what makes stale-quote
//! invalidation ([`model::invalidate`]) possible across a restart.
//!
//! [`telemetry`] is the write side: a tap that publishes what each tick
//! decided to the shared Postgres for the provisioned Grafana dashboards. It
//! is one-way on purpose — nothing in the quoting path reads it back, and a
//! database that is down or unmigrated degrades to "no telemetry", never to a
//! bot that will not quote.
//!
//! [`fx_store`] reads the *same* Postgres, and takes the opposite posture on
//! purpose. It is where the intraday FX anchor comes from — the collectors own
//! the keyed, metered FX venues and the maker reads their rows — so it is part
//! of price formation rather than a tap on it, and losing it **halts** the bot
//! instead of degrading it. The alternative is quoting off a daily ECB fix
//! while the data the operator chose to price off is simply missing, and at
//! this size not quoting is free.
//!
//! Two uses of one database with opposite failure postures is not an
//! inconsistency: the question is never "is Postgres up" but "is this fact
//! load-bearing for the price". A dropped telemetry row costs a gap in a
//! dashboard; a missing FX print costs a wrong quote.

pub mod chain;
pub mod config;
pub mod context;
pub mod fills;
pub mod fx_store;
pub mod model;
pub mod quote_state;
pub mod tasks;
pub mod telemetry;
