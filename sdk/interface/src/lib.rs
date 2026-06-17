//! `dropset-interface` — the solana-free, off-chain half of the eCLOB
//! shared math.
//!
//! Holds the on-chain market [`layout`] mirror and the just-in-time book
//! reconstruction in [`matching`] — the decode + simulation the IDL can't
//! describe (the `Vault` slab is opaque to it). Both build on the
//! consensus arithmetic in [`dropset_math_core`] (the [`Price`] codec and
//! the pure matcher math), so the simulator runs the engine's exact
//! numbers and a router quoting off it can't drift.
//!
//! Unlike `dropset-math-core`, **nothing here runs on-chain**: these
//! decode account bytes and simulate fills for routers, the `/orderbook`
//! depth endpoint, and the WASM client. A bug mis-predicts a quote rather
//! than corrupting state, so the audit priority is lower — parity is pinned
//! by the shared conformance vectors (see `sdk/conformance`), not by the
//! on-chain engine running this code.
//!
//! **Feature surface.** [`layout`] is always compiled. [`matching`]
//! (`simulate`, default on) pulls `std` collections for off-chain book
//! reconstruction. `wasm` adds the wasm-bindgen `simulate_swap` export and
//! turns on `dropset-math-core/wasm`, so one wasm-pack build over this
//! crate emits the full binding set (book simulator + `Price` codec).

// Re-export the math spine so downstream `dropset_interface::{price,
// matching_math}` paths resolve without a separate import, and so the
// `Price` codec the layout/matching modules use is the one canonical impl.
pub use dropset_math_core::{matching_math, price, price::Price};

pub mod layout;

#[cfg(feature = "simulate")]
pub mod matching;

#[cfg(feature = "wasm")]
pub mod wasm;
