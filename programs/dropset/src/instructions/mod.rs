pub mod admin;
pub mod close_vault;
pub mod create_market;
pub mod create_vault;
// anchor-next's `#[derive(Accounts)]` re-binds every `#[instruction(..)]`
// arg inside the generated `exit_accounts` as well as `validate_accounts`,
// and on a struct whose exit phase reads none of them that second binding
// is dead — `unused_variables` on the attribute's own span, even for an
// arg the seeds use. Lint levels for derive output come from the
// enclosing module, so the allow sits on each module it fires in.
#[allow(unused_variables)]
pub mod deposit;
pub mod deposit_leader;
pub mod freeze_vault;
pub mod init;
pub mod override_vault_name;
pub mod registry_defaults;
pub mod retune;
#[allow(unused_variables)] // see `deposit`
pub mod set_leader;
pub mod set_liquidity_profile;
pub mod set_outside_deposits;
pub mod set_quote_authority;
pub mod set_reference_price;
pub mod set_vault_name;
pub mod swap;
pub mod sweep_residual;
#[allow(unused_variables)] // see `deposit`
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
#[allow(unused_variables)] // see `deposit`
pub mod force_withdraw;

use anchor_lang::prelude::*;
use anchor_spl::{
    token_2022::{transfer_checked, TransferChecked},
    token_interface::TokenAccount,
};

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

/// Inbound single-leg transfer: move `amount` of one mint from the
/// signer's ATA into the market treasury via `transfer_checked`,
/// authorized by the signer itself (`CpiContext::new`, no PDA seeds).
///
/// Shared by `deposit`, `deposit_leader` and `swap`'s input leg so the
/// zero-skip, CPI shape and received-amount check stay identical across
/// all three. `transfer_checked` rejects zero
/// amounts on classic SPL Token, so a zero leg is skipped here rather
/// than at each call site. This is the **inbound** family (authority =
/// the signer); the outbound treasury→user pair uses
/// `new_with_signer` with the market PDA seeds.
///
/// Every caller books `amount`, so the treasury must receive exactly
/// `amount`: its balance is snapshotted before the CPI and compared
/// after, failing [`DropsetError::TransferShortfall`] otherwise.
/// `transfer_checked` validates the transfer's shape, not the quantity
/// delivered — a Token-2022 transfer-fee mint withholds part of it in
/// the destination, which would leave `treasury.amount()` below the
/// vault legs it backs. On the deposit paths, rejecting rather than
/// crediting the delta also keeps `single_leg_basket`'s VPS-preserving
/// basket exact. No `reload` is needed: `InterfaceAccount` reads through
/// to the live account data, so `treasury.amount()` after the CPI is the
/// post-transfer balance.
#[allow(clippy::too_many_arguments)]
pub fn transfer_in_leg<'a>(
    token_program: &'a Address,
    from_signer_ata: CpiHandleMut<'a>,
    mint: CpiHandle<'a>,
    treasury: &mut InterfaceAccount<TokenAccount>,
    authority: CpiHandle<'a>,
    amount: u64,
    decimals: u8,
) -> Result<()> {
    if amount == 0 {
        return Ok(());
    }
    let before = treasury.amount();
    let cpi = CpiContext::new(
        token_program,
        TransferChecked {
            from: from_signer_ata,
            mint,
            to: treasury.cpi_handle_mut(),
            authority,
        },
    );
    transfer_checked(cpi, amount, decimals)?;
    require!(
        treasury.amount().checked_sub(before) == Some(amount),
        DropsetError::TransferShortfall
    );
    Ok(())
}

/// Outbound single-leg payout transfer: move `amount` of one mint out of
/// a program-owned token account to `dest` via `transfer_checked`, signed
/// by that account's owning PDA (`CpiContext::new_with_signer` with
/// `signer_seeds`).
///
/// The **outbound** sibling of [`transfer_in_leg`] — shared by every
/// payout out of program custody, with the owning PDA as the signing
/// authority, so the zero-skip and CPI shape stay identical; only the
/// source, destination and signing PDA differ per call.
/// `transfer_checked` rejects zero amounts on classic SPL Token, so a
/// zero leg is skipped here rather than at each call site.
#[allow(clippy::too_many_arguments)]
pub fn transfer_out_leg<'a>(
    token_program: &'a Address,
    source: CpiHandleMut<'a>,
    mint: CpiHandle<'a>,
    dest: CpiHandleMut<'a>,
    authority: CpiHandle<'a>,
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
            from: source,
            mint,
            to: dest,
            authority,
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
pub use override_vault_name::*;
pub use registry_defaults::*;
pub use retune::*;
pub use set_leader::*;
pub use set_liquidity_profile::*;
pub use set_outside_deposits::*;
pub use set_quote_authority::*;
pub use set_reference_price::*;
pub use set_vault_name::*;
pub use swap::*;
pub use sweep_residual::*;
pub use withdraw::*;
pub use withdraw_leader::*;
