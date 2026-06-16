pub mod admin;
pub mod close_vault;
pub mod create_market;
pub mod create_vault;
pub mod deposit;
pub mod deposit_leader;
pub mod freeze_vault;
pub mod init;
pub mod set_liquidity_profile;
pub mod set_outside_deposits;
pub mod set_reference_price;
pub mod swap;
pub mod withdraw;
pub mod withdraw_leader;
// Teardown surface. The handlers always compile and are always wired
// into the program, but each dispatcher (see `lib.rs`) short-circuits to
// `DropsetError::TeardownDisabled` unless the `admin-teardown` Cargo
// feature is on — so testnet / early-mainnet builds expose them and the
// final immutable build leaves them present-but-inert. anchor v2's
// `#[program]` macro does not propagate `#[cfg]` from a handler fn onto
// its generated dispatch glue, so a clean per-instruction compile-out
// isn't available; the runtime guard is the supported alternative. See
// the architecture spec, § Account lifecycle and rent reclamation.
pub mod close_market;
pub mod close_registry;
pub mod force_withdraw;

use anchor_lang_v2::prelude::*;
use anchor_spl_v2::token_2022::{transfer_checked, TransferChecked};

/// Inbound single-leg deposit transfer: move `amount` of one mint from
/// the signer's ATA into the market treasury via `transfer_checked`,
/// authorized by the signer itself (`CpiContext::new`, no PDA seeds).
///
/// Shared by `deposit` and `deposit_leader` so the zero-skip and CPI
/// shape stay identical across both. `transfer_checked` rejects zero
/// amounts on classic SPL Token, so a zero leg is skipped here rather
/// than at each call site. This is the **inbound** family (authority =
/// the signer); the outbound treasury→user pair uses
/// `new_with_signer` with the market PDA seeds.
#[allow(clippy::too_many_arguments)]
pub fn transfer_in_leg<'a>(
    token_program: &'a Address,
    from_signer_ata: CpiHandleMut<'a>,
    mint: CpiHandle<'a>,
    treasury: CpiHandleMut<'a>,
    authority: CpiHandle<'a>,
    amount: u64,
    decimals: u8,
) -> core::result::Result<(), ProgramError> {
    if amount == 0 {
        return Ok(());
    }
    let cpi = CpiContext::new(
        token_program,
        TransferChecked {
            from: from_signer_ata,
            mint,
            to: treasury,
            authority,
        },
    );
    transfer_checked(cpi, amount, decimals)
}

/// Outbound single-leg payout transfer: move `amount` of one mint from
/// the market treasury to `dest` via `transfer_checked`, signed by the
/// market PDA (`CpiContext::new_with_signer` with `signer_seeds`).
///
/// The **outbound** sibling of [`transfer_in_leg`] — shared by every
/// treasury→holder payout (both `withdraw` legs, both `withdraw_leader`
/// legs, and all four `force_withdraw` legs) so the zero-skip and CPI
/// shape stay identical; only the destination ATA differs per call.
/// `transfer_checked` rejects zero amounts on classic SPL Token, so a
/// zero leg is skipped here rather than at each call site.
#[allow(clippy::too_many_arguments)]
pub fn transfer_out_leg<'a>(
    token_program: &'a Address,
    treasury: CpiHandleMut<'a>,
    mint: CpiHandle<'a>,
    dest: CpiHandleMut<'a>,
    market_authority: CpiHandle<'a>,
    amount: u64,
    decimals: u8,
    signer_seeds: &[&[&[u8]]],
) -> core::result::Result<(), ProgramError> {
    if amount == 0 {
        return Ok(());
    }
    let cpi = CpiContext::new_with_signer(
        token_program,
        TransferChecked {
            from: treasury,
            mint,
            to: dest,
            authority: market_authority,
        },
        signer_seeds,
    );
    transfer_checked(cpi, amount, decimals)
}

pub use admin::*;
pub use close_market::*;
pub use close_registry::*;
pub use close_vault::*;
pub use create_market::*;
pub use create_vault::*;
pub use deposit::*;
pub use deposit_leader::*;
pub use force_withdraw::*;
pub use freeze_vault::*;
pub use init::*;
pub use set_liquidity_profile::*;
pub use set_outside_deposits::*;
pub use set_reference_price::*;
pub use swap::*;
pub use withdraw::*;
pub use withdraw_leader::*;
