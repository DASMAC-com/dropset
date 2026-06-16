//! `dropset-price-core` — the canonical arithmetic spine of the eCLOB.
//!
//! Holds the [`Price`](price::Price) codec, the pure matcher math in
//! [`matching_math`] (flush-level pricing, the size-bps fill cap, the
//! price-time sort key), the on-chain market [`layout`] mirror, and the
//! just-in-time book reconstruction in [`matching`] — the math the IDL
//! can't describe (the `Vault` slab is opaque to it).
//!
//! This crate is the single home for the `Price` codec and the consensus
//! pieces of the matcher: the on-chain program (`programs/dropset`) depends
//! on it directly and the off-chain SDK / WASM client share the exact same
//! code, so the live engine and any router quoting off the simulator can no
//! longer drift.
//!
//! **Feature surface.** The codec and [`matching_math`] are solana-free and
//! always compiled. [`matching`] (`simulate`, default on) pulls `std`
//! collections for off-chain book reconstruction; the on-chain program
//! turns it off. The Anchor `IdlType` derive on `Price` is gated behind
//! `idl` (which pulls the non-solana-free `anchor-lang-v2`) and must never
//! be combined with `wasm`.
//!
//! Correctness is pinned to the on-chain engine by the shared conformance
//! vectors in `sdk/conformance`, verified in both Rust and TS — the TS
//! `price.ts` is the one remaining intentional cross-language re-impl.

pub mod layout;
pub mod matching_math;
pub mod price;

pub use price::Price;

#[cfg(feature = "simulate")]
pub mod matching;

#[cfg(feature = "wasm")]
pub mod wasm;
