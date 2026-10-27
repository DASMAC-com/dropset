//! Titan router adapter — `Titan-Pathfinder/integration-template`.
//!
//! Titan venues implement the `TradingVenue` trait. It maps onto
//! [`DropsetAmm`] more directly than the Jupiter/DFlow `Amm` trait — there
//! is **no closed `Swap` enum**, since `generate_swap_instruction` returns
//! the venue program's own instruction:
//!
//! | `TradingVenue` method        | [`DropsetAmm`]                       |
//! | ---------------------------- | ------------------------------------ |
//! | `initialized`                | always true once `from_account` succeeds |
//! | `from_account`               | [`DropsetAmm::from_account`]         |
//! | `update_state`               | [`DropsetAmm::update`]               |
//! | `get_token_info`             | [`DropsetAmm::leg_info`]             |
//! | `quote`                      | [`DropsetAmm::quote`] (zero-input-safe) |
//! | `generate_swap_instruction`  | [`DropsetAmm::swap_instruction`]     |
//!
//! Titan's framework discovers fill bounds by calling `quote` with an
//! exponential/binary search, so it requires a **zero-input-safe** quote —
//! [`DropsetAmm::quote`] returns an empty [`DropsetQuote`] for `amount == 0`
//! (covered by a unit test in [`super::amm`]). Token-2022 legs settle
//! through the venue program's `transfer_checked` CPIs, with the per-leg
//! token program carried in [`DropsetSwapParams`]. On a transfer-fee
//! mint the venue rejects the swap when that leg is the input
//! (`TransferShortfall`: the treasury must receive the full amount), and
//! delivers less than the quote when it is the output, net of the fee.
//!
//! ## Wiring to the upstream crate
//!
//! Only the solana-crate versions need pinning to Titan's `Cargo.toml`
//! (the template is split-crate + LiteSVM, recent majors); `Pubkey` /
//! `Instruction` convert at the boundary via raw bytes if a major differs.

pub use super::amm::{
    AmmError, DropsetAmm, DropsetQuote, DropsetQuoteParams, DropsetSwapParams, LegInfo,
    CLOCK_SYSVAR,
};
