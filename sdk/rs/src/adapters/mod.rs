//! Router / aggregator integration adapters.
//!
//! All off-chain router traits (Jupiter `Amm`, DFlow `Amm`, Titan
//! `TradingVenue`) want the same shape — build from an account, list
//! accounts to refresh, quote off-chain, emit a swap instruction — so the
//! router-agnostic core lives in [`amm`] ([`amm::DropsetAmm`]) and each
//! router module is a thin trait-mapping over it (book state via
//! [`crate::layout`], quotes via [`crate::matching`], swap ix via the
//! generated [`crate::instructions`] builder). See interface.md § 4 / § 6.
//!
//! - [`jupiter`] / [`dflow`] — the `Amm` trait. Drop-in upstream impls are
//!   gated on solana-crate version skew + each router's closed `Swap` enum.
//! - [`titan`] — the `TradingVenue` trait. Closest to drop-in (no closed
//!   enum; `generate_swap_instruction` returns the program's own ix).
//!
//! beethoven CPI composability is **not** here: it's an on-chain Pinocchio
//! integration, so per interface.md §6 it belongs in the future `no_std`,
//! entrypoint-free CPI crate (the on-chain CPI tier, a sibling of
//! `dropset-math-core`), not this off-chain SDK and not `dropset-interface`
//! (which is off-chain too). It's blocked on a swap-context extension.

pub mod amm;
pub mod dflow;
pub mod jupiter;
pub mod titan;

pub use amm::DropsetAmm;
