//! Per-market state: [`MarketHeader`] + a [`Slab`]-tail of [`Vault`]
//! sectors threaded into three doubly-linked lists (active / tombstoned /
//! free). See the architecture spec's **MarketHeader**, **Storage layout**,
//! and **Vault** sections.
//!
//! The module is split by concern, each in its own submodule:
//! - [`layout`] — the byte-exact `#[repr(C)]` / `#[account]` types plus the
//!   size / offset const-asserts that pin their on-chain layout.
//! - [`access`] — [`VaultAccess`]: bounds-checked sector borrows.
//! - [`dll`] — [`VaultDll`] / [`DllList`]: the doubly-linked-list surgery
//!   over the active / tombstone / free sector lists.
//! - [`accrual`] — perf-fee realization ([`realize_in_place`]) and
//!   single-leg subsequent-deposit sizing ([`single_leg_basket`]).
//!
//! Every public name is glob-re-exported here, so `crate::state::*` and the
//! crate-root `crate::{Vault, MarketHeader, …}` re-exports resolve exactly
//! as they did when this was one file.

use anchor_lang_v2::accounts::Slab;

mod access;
mod accrual;
mod dll;
mod layout;
mod reference_price;

pub use access::*;
pub use accrual::*;
pub use dll::*;
pub use layout::*;
pub use reference_price::*;

// The pure seeding / withdrawal kernels are solana-free, so they live in
// `dropset-math-core` and are re-exported here unchanged — every
// `crate::state::{isqrt_u128, compute_pro_rata_slice}` call site keeps
// resolving, and the on-chain program runs byte-identical math to the
// off-chain consumers. The perf-fee accrual and single-leg sizing keep a
// thin wrapper in `accrual` (one maps the math-core error back onto
// `DropsetError`, the other reads/writes `&mut Vault` state around the pure
// formula).
pub use dropset_math_core::share::{compute_pro_rata_slice, isqrt_u128};

/// Number of bid / ask levels in a [`LiquidityProfile`]. Chosen small for
/// the initial bring-up; widen once the matching engine lands and CU
/// budgets are measured.
pub const N_LEVELS: usize = 8;

/// "Null" sentinel for sector-index pointers ([`MarketHeader::head`],
/// [`Vault::next`], etc.). Sector indices are at most
/// `registry.max_vaults_per_market` (`u8`), so [`u32::MAX`] is unreachable
/// as a real index and works as a null marker.
pub const NULL_SECTOR: u32 = u32::MAX;

/// Flush flag OR'd onto [`ReferencePrice::stamp`] by `SetReferencePrice`
/// and `SetLiquidityProfile`. The next taker materializes
/// [`Vault::remaining`] from the [`LiquidityProfile`] and clears the
/// flag — see the spec's **LiquidityProfile → Flush**.
pub const FLUSH_BIT: u64 = 1u64 << 63;

/// Q32.32 fixed-point representation of `1.0` — the seed value for
/// [`Vault::hwm`] at first-deposit time. The HWM is value-per-share
/// (`L / total_shares`); the first depositor's basket implies
/// `L = total_shares` (since `total_shares := isqrt(b·q) == L`), so the
/// initial VPS is exactly 1.0.
pub const Q32_32_ONE: u64 = 1u64 << 32;

/// Parts-per-million denominator (`1_000_000 = 100%`).
pub const PPM: u64 = 1_000_000;

/// Basis-points denominator (`10_000 = 100%`).
pub const BPS: u64 = 10_000;

/// Market account: [`MarketHeader`] followed by a slab tail of [`Vault`]
/// sectors. Sectors are managed via the [`VaultDll`] operations rather
/// than the raw slab `push` / `swap_remove` — those would break the DLL
/// invariants the matching engine relies on.
pub type Market = Slab<MarketHeader, Vault>;

#[cfg(test)]
mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_for_zero_matches_init_space() {
        // `space_for(0)` is the byte count used by `create_market`'s
        // `#[account(init, space = Market::space_for(0))]`; `INIT_SPACE`
        // is the same value surfaced through the `Space` trait that
        // anchor's derive consults during initialization. Pinning their
        // equality here guarantees the two paths stay in sync.
        assert_eq!(
            Market::space_for(0),
            <Market as anchor_lang_v2::Space>::INIT_SPACE
        );
    }
}
