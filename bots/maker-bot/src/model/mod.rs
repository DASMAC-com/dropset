//! The bot's quoting model — the pure decision logic of
//! `docs/market-making-mvp.md`, independent of the chain and the network.
//!
//! A tick flows through these in order: poll [`feeds`] → compose [`fair_mid`]
//! → value [`inventory`] → compute the [`skew`] → decide the [`killswitch`]
//! action and the [`triggers`] cadence → build the [`ladder`]. Everything here
//! is deterministic and unit tested; the I/O lives in `chain` and `tasks`.

pub mod fair_mid;
pub mod feeds;
pub mod inventory;
pub mod killswitch;
pub mod ladder;
pub mod skew;
pub mod triggers;
