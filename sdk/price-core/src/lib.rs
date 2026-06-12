//! `dropset-price-core` — the solana-free arithmetic spine of the SDK.
//!
//! Holds the [`Price`](price::Price) codec, the on-chain market [`layout`]
//! mirror, and the just-in-time book reconstruction in [`matching`] — the
//! math the IDL can't describe (the `Vault` slab is opaque to it). It has
//! no Solana dependency, so the Rust SDK uses it directly and the same code
//! compiles to WASM (the `wasm` feature) for the TypeScript/Python clients,
//! instead of each language hand-mirroring the engine.
//!
//! Correctness is pinned to the on-chain engine by the shared conformance
//! vectors in `sdk/conformance`, verified in both Rust and TS.

pub mod layout;
pub mod matching;
pub mod price;

#[cfg(feature = "wasm")]
pub mod wasm;
