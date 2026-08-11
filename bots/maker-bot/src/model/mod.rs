//! The bot's quoting model — the pure decision logic of
//! `docs/market-making.md`, independent of the chain and the network.
//!
//! A tick flows through these in order: poll [`feeds`] → compose [`fair_mid`]
//! → value [`inventory`] → compute the [`skew`] → decide the [`killswitch`]
//! action and the [`triggers`] cadence → build the [`ladder`]. Everything here
//! is deterministic and unit tested; the I/O lives in `chain` and `tasks`.
//!
//! [`invalidate`] sits outside that flow — it runs when the flow *stops* (a
//! restart, a dark feed, a halt) and decides whether the quotes left resting
//! must be killed rather than left to expire.

pub mod fair_mid;
pub mod feeds;
pub mod invalidate;
pub mod inventory;
pub mod killswitch;
pub mod ladder;
pub mod skew;
pub mod triggers;
