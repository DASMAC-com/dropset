//! Jupiter router adapter — `jup-ag/jupiter-amm-interface`.
//!
//! DFlow's interface is a fork of Jupiter's, so the `Amm` trait shape is
//! identical and maps onto [`DropsetAmm`] exactly as in [`super::dflow`].
//!
//! ## Wiring to the upstream crate
//!
//! 1. **Type skew (lighter than DFlow's).** `jupiter-amm-interface` uses
//!    the split crates — `solana-pubkey 4.0`, `solana-instruction 3.1`,
//!    `solana-account 3.4`, `solana-clock 3.0` (edition 2024). Our
//!    `solana-instruction` major matches (3.x), so only `Pubkey` (major 4
//!    vs our 3) needs the byte-level conversion at the boundary; aligning
//!    this SDK to `solana-pubkey 4` would remove even that.
//! 2. **`Swap` enum.** Jupiter's `Swap` enum is closed (the long-standing
//!    "add-a-variant" onboarding step). A Dropset variant must be added
//!    upstream before `get_swap_and_account_metas` can return one; until
//!    then [`DropsetAmm::swap_instruction`] returns the instruction +
//!    account metas directly.
//!
//! `Amm::quote` also wants `fee_amount` / `fee_mint`: the taker fee is the
//! output-leg [`DropsetQuote::fee_amount`], denominated in the output mint
//! (base for a buy, quote for a sell).

pub use super::amm::{
    AmmError, DropsetAmm, DropsetQuote, DropsetQuoteParams, DropsetSwapParams, CLOCK_SYSVAR,
};
