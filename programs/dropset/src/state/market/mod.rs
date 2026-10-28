//! Per-market state: [`MarketHeader`] + a [`Slab`]-tail of [`Vault`]
//! sectors threaded into three doubly-linked lists (active / tombstoned /
//! free). See the architecture spec's **MarketHeader**, **Storage layout**,
//! and **Vault** sections.
//!
//! The module is split by concern, each in its own submodule:
//! - `layout` — the byte-exact `#[repr(C)]` / `#[account]` types plus the
//!   size / offset const-asserts that pin their on-chain layout.
//! - `access` — [`VaultAccess`]: bounds-checked sector borrows.
//! - `dll` — [`VaultDll`] / [`DllList`]: the doubly-linked-list surgery
//!   over the active / tombstone / free sector lists.
//! - `accrual` — the thin wrappers over `dropset-math-core` kernels:
//!   perf-fee realization ([`realize_in_place`]), single-leg
//!   subsequent-deposit sizing ([`single_leg_basket`]), and the quote
//!   flush ([`Vault::materialize_remaining`]).
//! - `inventory` — the deposit's and withdraw's inventory + share
//!   mutations ([`apply_deposit_inventory`], [`apply_withdraw_inventory`])
//!   and the resting-size rescale they share ([`rescale_remaining`]),
//!   which wrap no kernel.
//! - `name` — [`Vault::name`] validation and packing
//!   ([`encode_vault_name`]), shared by every name writer.
//! - `quote_write` / `reference_price` / `liquidity_profile` — the two
//!   solana-free quote-write kernels the hand-written sBPF entrypoint
//!   mirrors ([`stamp_reference_price`], [`write_liquidity_profile`]), over
//!   the preamble and byte accessors they share.
//!
//! Every public name is glob-re-exported here, so `crate::state::*` and the
//! crate-root `crate::{Vault, MarketHeader, …}` re-exports resolve exactly
//! as they did when this was one file.

use anchor_lang::accounts::Slab;

mod access;
mod accrual;
mod dll;
mod inventory;
// `pub(crate)` because `anchor-asm-v2` names each offset-exported type by
// its defining path (`crate::state::market::layout::Vault`), not by the
// glob re-export below, in the `combined.rs` it generates into the crate.
pub(crate) mod layout;
mod liquidity_profile;
mod name;
mod quote_write;
mod reference_price;

pub use access::*;
pub use accrual::*;
pub use dll::*;
pub use inventory::*;
pub use layout::*;
pub use liquidity_profile::*;
pub use name::*;
pub use quote_write::err;
pub use reference_price::*;

// The pure seeding / withdrawal / floor kernels are solana-free, so they
// live in `dropset-math-core` and are re-exported here unchanged — every
// `crate::state::{isqrt_u128, compute_pro_rata_slice, min_leader_share_ok}`
// call site keeps resolving, and the on-chain program runs byte-identical
// math to the off-chain consumers. The perf-fee accrual and single-leg
// sizing keep a thin wrapper in `accrual` (one reads/writes `&mut Vault`
// state around the pure formula, the other maps the math-core error back
// onto `DropsetError`); the depositor-side kernels are wrapped on
// `VaultDepositorHeader`.
pub use dropset_math_core::share::{compute_pro_rata_slice, isqrt_u128, min_leader_share_ok};

/// Byte width of [`Vault::name`]: at most this many printable-ASCII
/// bytes, zero-padded.
pub const VAULT_NAME_LEN: usize = 32;

// The layout sentinels (level count, null sector, flush flag, Q32.32 one),
// the Q32.32 shift width, the per-swap leg cap and the parts-per-million
// and basis-points denominators are re-exported from math-core rather than redeclared, for
// the same reason as the kernels above: the off-chain layout mirror, the
// bounds this crate checks and the divisions math-core performs must all
// read the same constant, not two literals that happen to agree.
pub use dropset_math_core::{
    BPS, FLUSH_BIT, MAX_LEGS_PER_SWAP, NULL_SECTOR, N_LEVELS, PPM, Q32_32_ONE, Q32_SHIFT,
};

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
            <Market as anchor_lang::Space>::INIT_SPACE
        );
    }
}
