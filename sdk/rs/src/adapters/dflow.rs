//! DFlow router adapter — `DFlowProtocol/dflow-amm-interface` (a fork of
//! `jupiter-amm-interface`).
//!
//! Maps DFlow's `Amm` trait onto [`DropsetAmm`]:
//!
//! | `Amm` method                  | [`DropsetAmm`]            |
//! | ----------------------------- | ------------------------ |
//! | `from_keyed_account`          | [`DropsetAmm::from_account`] |
//! | `program_id` / `key`          | `program_id` / `key`     |
//! | `get_reserve_mints`           | `reserve_mints`          |
//! | `get_accounts_to_update`      | `accounts_to_update`     |
//! | `update`                      | `update`                 |
//! | `quote`                       | `quote`                  |
//! | `get_swap_and_account_metas`  | `swap_instruction`       |
//!
//! ## Wiring to the upstream crate
//!
//! Two external boundary items keep a drop-in `impl Amm for DropsetAmm`
//! out of this crate (neither blocks the quoting logic above):
//!
//! 1. **Type skew.** DFlow pins `solana-sdk = "=2.3.*"` (the monolith),
//!    whereas this SDK uses the split `solana-pubkey`/`solana-instruction`
//!    3.x crates. They coexist in one tree (different majors); the shim
//!    converts `Pubkey`/`Instruction` at the boundary via raw bytes
//!    (`Pubkey::to_bytes()` round-trips), so no logic changes.
//! 2. **`Swap` enum.** `SwapAndAccountMetas.swap` is DFlow's closed `Swap`
//!    enum; emitting one for Dropset needs a variant added upstream in
//!    DFlow's fork (an upstream, instruction-level dependency).
//!    Until then [`DropsetAmm::swap_instruction`] returns the ready-built
//!    instruction + account metas directly.

pub use super::amm::{
    AmmError, DropsetAmm, DropsetQuote, DropsetQuoteParams, DropsetSwapParams, CLOCK_SYSVAR,
};
