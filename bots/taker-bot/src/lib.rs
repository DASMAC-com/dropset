//! `dropset-taker-bot` — the localnet CADC/USDC flow-generation taker.
//!
//! A single taker bot that submits stochastic `swap`s against the mock
//! CADC/USDC market (`docs/market-making-mvp.md`) so the book actually moves
//! and the maker takes fills on localnet. It is the **benign** flow taker —
//! distinct from the adversarial strategy-hardening taker, which is a
//! separate, deferred effort.
//!
//! The order flow models dropset-alpha's taker: a two-state (quiet / burst)
//! Markov chain feeds a Poisson arrival count per tick, order sizes are
//! LogNormal, and the buy-bias mean-reverts toward 0.5 each order. With a
//! fixed RNG seed the whole flow replays deterministically.
//!
//! The crate follows the maker-bot's split:
//!
//! - [`config`] — every knob, with MVP defaults.
//! - [`model`] — the pure, seedable stochastic process, unit tested.
//!
//! The [`context`], [`chain`], and [`tasks`] modules layer the runtime state,
//! on-chain I/O (discovery, self-funding, order sizing via the off-chain
//! simulator, and the swap send), and the tick loop on top of that core.

pub mod chain;
pub mod config;
pub mod context;
pub mod model;
pub mod tasks;
