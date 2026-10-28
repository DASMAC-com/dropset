//! `dropset-math-core` — the canonical, consensus-critical arithmetic
//! spine of the eCLOB.
//!
//! Holds the [`Price`] codec, the pure matcher math in [`matching_math`]
//! (flush-level pricing, the size-bps fill cap, the price-time sort key),
//! the share/NAV/PnL accounting kernels in
//! [`share`] (the seeding `isqrt`, single-leg deposit sizing, the
//! pro-rata withdrawal slice, the perf-fee accrual formula, and the
//! realized-PnL crystallization), and the two expiry clock domains in
//! [`clock`] — the `repr(transparent)` newtypes that keep a slot count
//! and a unix second from being confused for one another, plus the one
//! definition of the zero-is-dead deadline rule both matchers call.
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
//! is gated behind `idl` (which pulls the non-solana-free `anchor-lang`)
//! and must never be combined with `wasm`.
//!
//! Correctness is pinned to the on-chain engine by the shared conformance
//! vectors in `sdk/conformance`, verified in both Rust and TS — the TS
//! `price.ts` and `share.ts` are intentional hand ports, held to this
//! crate only by those vectors.
//!
//! **Layout sentinels.** [`N_LEVELS`], [`NULL_SECTOR`], [`FLUSH_BIT`] and
//! [`Q32_32_ONE`] are not arithmetic, but they live here too: this is the
//! only crate both the program and `dropset-interface` depend on, so it is
//! the one place a constant both sides must agree on can be declared once.

pub mod clock;
pub mod matching_math;
pub mod price;
pub mod share;

pub use clock::{SlotSpan, SlotTime, WallSpan, WallTime};
pub use price::Price;

/// Parts-per-million denominator (`1_000_000 = 100%`) — the scale for
/// flush price offsets and the perf-fee rate.
pub const PPM: u64 = 1_000_000;

/// Basis-points denominator (`10_000 = 100%`) — the scale for per-level
/// flush sizes.
pub const BPS: u64 = 10_000;

/// Fractional-bit width of the Q32.32 fixed point that value-per-share and
/// the high-water mark are stored in: `x << Q32_SHIFT` lifts an atom-scale
/// value into Q32.32, and `>> Q32_SHIFT` brings it back. Distinct from
/// [`Q32_32_ONE`], which is `1.0` in this encoding.
pub const Q32_SHIFT: u32 = 32;

/// Q32.32 fixed-point representation of `1.0` — the seed value for a
/// vault's `hwm` at first-deposit time. The HWM is value-per-share
/// (`L / total_shares`); the first depositor's basket implies
/// `L = total_shares` (since `total_shares := isqrt(b·q) == L`), so the
/// initial VPS is exactly 1.0.
pub const Q32_32_ONE: u64 = 1u64 << Q32_SHIFT;

/// Number of bid / ask levels in a `LiquidityProfile`. Chosen small for
/// the initial bring-up; widen once CU budgets are measured.
pub const N_LEVELS: usize = 8;

/// Most `(vault, level)` legs one `swap` fills. The walk stops at the cap
/// the way it stops on an exhausted budget: the taker keeps the unspent
/// remainder and the `min_out` soft-revert still runs, so a book deeper
/// than one transaction can settle gives a bounded partial fill (or a clean
/// soft-revert), never a transaction failure. The engine and the off-chain
/// simulator both read it from here, so a quote and its execution stop on
/// the same leg.
///
/// What it guards is the program's 32 KiB bump heap, which never frees:
/// each filled leg buffers a compact record and, at emit time, costs the
/// `emit_cpi!` path a few hundred bytes more. Without the cap the leg count
/// is attacker-shaped — one vault rests up to `2 × N_LEVELS` entries (own
/// plus flipped), so a dust-quoting vault ahead of the honest book could
/// push every take past the heap into a VM abort. Sized from the litesvm
/// measurement in `programs/dropset/tests/flip.rs`, which pins that a take
/// at the cap completes on a fully flipped book at the default vault cap.
pub const MAX_LEGS_PER_SWAP: u32 = 32;

/// "Null" sentinel for sector-index pointers (`MarketHeader::head`,
/// `Vault::next`, `Vault::prev`). Sector indices are at most
/// `registry.max_vaults_per_market` (`u8`), so [`u32::MAX`] is unreachable
/// as a real index and works as a null marker.
pub const NULL_SECTOR: u32 = u32::MAX;

/// Flush flag OR'd onto `ReferencePrice::stamp` by `SetReferencePrice`
/// and `SetLiquidityProfile`. The next taker materializes
/// `Vault::remaining` from the `LiquidityProfile` and clears the flag —
/// see the spec's **LiquidityProfile → Flush**.
pub const FLUSH_BIT: u64 = 1u64 << 63;

#[cfg(feature = "wasm")]
pub mod wasm;
