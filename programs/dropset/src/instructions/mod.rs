pub mod admin;
pub mod close_vault;
pub mod create_market;
pub mod create_vault;
pub mod deposit;
pub mod deposit_leader;
pub mod freeze_vault;
pub mod init;
pub mod registry_defaults;
pub mod retune;
pub mod set_liquidity_profile;
pub mod set_outside_deposits;
pub mod set_quote_authority;
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

use anchor_lang_v2::{prelude::*, AnchorAccount};
use anchor_spl_v2::token_2022::{transfer_checked, TransferChecked};

use crate::{errors::DropsetError, state::Market, AdminSet, Registry, VaultDepositorHeader};

/// Registry-admin precondition shared by every admin-gated instruction.
/// Rejects with [`DropsetError::Unauthorized`] unless `admin` is a member
/// of the registry admin set.
///
/// The check is a set-membership scan over the registry slab tail
/// ([`AdminSet::admin_contains`]), so it genuinely cannot be a declarative
/// `address` / `has_one` constraint (those do single-field equality only).
/// Hoisting it here lets each dispatcher declare the gate once — via
/// `#[access_control]` for the always-on levers, or the feature-on arm for
/// the teardown surface — instead of restating the same `require!` at the
/// top of every handler body, mirroring how `init` pins its precondition
/// through `Init::verify_upgrade_authority` (`lib.rs::init`).
pub fn require_registry_admin(registry: &Registry, admin: &Signer) -> Result<()> {
    require!(
        registry.admin_contains(admin.address()),
        DropsetError::Unauthorized
    );
    Ok(())
}

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

/// Close a `VaultDepositor` PDA and decrement the market's
/// `outstanding_vault_depositors` counter — the two must always move
/// together. That counter is the spec's only on-chain witness that
/// `close_market` can safely proceed (no orphan depositor PDAs remain,
/// since the program can't enumerate all PDAs), so a close that skips
/// the decrement — or vice versa — would break `close_market`
/// reachability. Rent is refunded to `refund_to`.
///
/// Shared by `withdraw` (under its `shares == 0` guard) and
/// `force_withdraw_depositor` (unconditional, full drain). The refund
/// recipient — the signer on the signed path, the position `owner` on
/// the force path — is the caller's choice. See the architecture spec,
/// § Account lifecycle and rent reclamation.
pub fn close_depositor_and_decrement(
    market: &mut Market,
    vault_depositor: &mut Account<VaultDepositorHeader>,
    refund_to: AccountView,
) -> Result<()> {
    vault_depositor.close(refund_to)?;
    let prev = market.outstanding_vault_depositors.get();
    market.outstanding_vault_depositors = prev.saturating_sub(1).into();
    Ok(())
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
pub use registry_defaults::*;
pub use retune::*;
pub use set_liquidity_profile::*;
pub use set_outside_deposits::*;
pub use set_quote_authority::*;
pub use set_reference_price::*;
pub use swap::*;
pub use withdraw::*;
pub use withdraw_leader::*;
