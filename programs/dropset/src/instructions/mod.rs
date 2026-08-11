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
pub mod sweep_residual;
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

/// Outbound single-leg payout transfer: move `amount` of one mint out of
/// a program-owned token account to `dest` via `transfer_checked`, signed
/// by that account's owning PDA (`CpiContext::new_with_signer` with
/// `signer_seeds`).
///
/// The **outbound** sibling of [`transfer_in_leg`] — shared by every
/// payout out of program custody (both `withdraw` legs, both
/// `withdraw_leader` legs, all four `force_withdraw` legs,
/// `sweep_residual`, and the drain-before-close in both
/// `close_market_treasury` and `close_registry_fee_vault`) so the
/// zero-skip and CPI shape stay identical; only the source, destination
/// and signing PDA differ per call. Nearly every caller passes a market
/// treasury signed by the market PDA; `close_registry_fee_vault` passes
/// the registry fee ATA signed by the registry PDA, which is why
/// `authority` is not market-specific.
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

/// Sum one leg's vault inventory across the market's **whole slab** —
/// the atoms the treasury holds that some depositor or leader still has a
/// claim on. `is_base` selects the leg.
///
/// Every sector is counted, not just the active DLL: a tombstoned vault
/// still holds depositor claims, and a reclaimed (free-list) sector could
/// in principle carry rounding dust. Counting every sector can only
/// *overstate* the claim, which errs toward leaving atoms in custody.
/// Cheap enough for the cold admin paths that use it —
/// `max_vaults_per_market` is a `u8`, so this is at most 255 iterations
/// (default 10).
///
/// Shared by `sweep_residual` (which subtracts it, plus the accrued fee,
/// to find the unclaimed residual) and `close_market_treasury` (which
/// requires it zero before draining the rest), so the whole-slab rule
/// can't drift between the instruction that measures a claim and the one
/// that acts on its absence. Saturating, and `u128` so a corrupt slab
/// claiming more than `u64::MAX` can't wrap into a small number.
pub fn leg_vault_sum(market: &Market, is_base: bool) -> u128 {
    let mut sum: u128 = 0;
    for v in market.as_slice() {
        let leg = if is_base {
            v.base_atoms.get()
        } else {
            v.quote_atoms.get()
        };
        sum = sum.saturating_add(leg as u128);
    }
    sum
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
pub use sweep_residual::*;
pub use withdraw::*;
pub use withdraw_leader::*;
