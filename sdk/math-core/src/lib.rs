//! `dropset-math-core` — the canonical, consensus-critical arithmetic
//! spine of the eCLOB.
//!
//! Holds the [`Price`](price::Price) codec, the pure matcher math in
//! [`matching_math`] (flush-level pricing, the size-bps fill cap, the
//! price-time sort key), and the share/NAV/PnL accounting kernels in
//! [`share`] (the seeding `isqrt`, single-leg deposit sizing, the
//! pro-rata withdrawal slice, the perf-fee accrual formula, and the
//! realized-PnL crystallization).
//!
//! Everything here **runs on-chain**: the program (`programs/dropset`)
//! depends on this crate directly and the off-chain SDK / WASM client share
//! the exact same code, so the live engine and any router quoting off the
//! simulator can no longer drift. The off-chain-only pieces — the account
//! layout mirror and the just-in-time book reconstruction that decode raw
//! account bytes — live in `dropset-interface`, which depends on this crate.
//!
//! **Feature surface.** The codec, [`matching_math`], and [`share`] are
//! solana-free and always compiled. The Anchor `IdlType` derive on `Price`
//! is gated behind `idl` (which pulls the non-solana-free `anchor-lang-v2`)
//! and must never be combined with `wasm`.
//!
//! Correctness is pinned to the on-chain engine by the shared conformance
//! vectors in `sdk/conformance`, verified in both Rust and TS — the TS
//! `price.ts` is the one remaining intentional cross-language re-impl.

pub mod matching_math;
pub mod price;
pub mod share;

pub use price::Price;

/// Parts-per-million denominator (`1_000_000 = 100%`) — the scale for
/// flush price offsets and the perf-fee rate.
pub const PPM: u64 = 1_000_000;

/// Basis-points denominator (`10_000 = 100%`) — the scale for per-level
/// flush sizes.
pub const BPS: u64 = 10_000;

#[cfg(feature = "wasm")]
pub mod wasm;
