//! `dropset-maker-bot` — the localnet CADC/USDC market-maker.
//!
//! A single leader bot that quotes CADC against USDC on the eCLOB per
//! `docs/market-making-mvp.md`. It polls external price feeds, composes a fair
//! mid from the two CADC sources (with the Oanda FX feed as a peg sanity
//! bound), and drives the program's relative-quoting hot path
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
//! The `context`, `chain`, and `tasks` modules (runtime state, on-chain I/O,
//! and the 5-second tick loop) build on this core.

pub mod config;
pub mod model;
