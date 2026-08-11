//! `close_registry_fee_vault` / `close_registry` — registry-side rent
//! reclamation, the final steps of the teardown / redeploy cycle.
//!
//! Both live behind the `admin-teardown` Cargo feature (see the
//! architecture spec's **Account lifecycle and rent reclamation**) and
//! are absent from the final immutable build.
//!
//! Order (once every market is closed): `close_registry_fee_vault` per
//! fee ATA → `remove_admin` down to the last admin → `close_registry`.
//! After the registry is closed the program holds zero on-chain state and
//! the upgrade authority can redeploy a fresh binary at the same id.
//!
//! `close_registry_fee_vault` drains the vault to a caller-supplied token
//! account before closing it, for the same reason
//! `close_market_treasury` does: there is no *other* instruction that
//! moves tokens out of a registry fee ATA, so demanding an empty account
//! would make a single collected `create_market` fee — or any unsolicited
//! transfer — permanently block the close, and with it the redeploy.

use anchor_lang_v2::{prelude::*, AnchorAccount};
#[allow(unused_imports)]
use anchor_spl_v2::{
    token_2022::{close_account, CloseAccount},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{errors::DropsetError, Registry};

use super::transfer_out_leg;

// ── close_registry_fee_vault ──────────────────────────────────────────

#[derive(Accounts)]
pub struct CloseRegistryFeeVault {
    /// Registry admin — authorized via the registry admin set.
    pub admin: Signer,
    /// Singleton registry. Read-only — the registry account itself is
    /// closed separately by `close_registry`. The registry PDA signs the
    /// `CloseAccount` CPI via its `[b"registry"]` seed.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Mint of the fee vault being closed. The program may have charged
    /// fees in more than one mint over its life; the admin closes each
    /// fee ATA in turn (the historical set is tracked off-chain).
    pub fee_mint: InterfaceAccount<Mint>,
    /// Token program owning `fee_mint`.
    pub token_program: Interface<'static, TokenInterface>,
    /// The fee ATA to close — pinned to `ata(registry, fee_mint,
    /// token_program)` by the constraint. Drained to `token_recipient`
    /// below, then closed.
    #[account(
        mut,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = token_program,
    )]
    pub fee_vault: InterfaceAccount<TokenAccount>,
    /// Receives the fee vault's collected **tokens** immediately before
    /// the close — the market-creation fees this vault accumulated, plus
    /// anything transferred to it unsolicited. Any admin-chosen token
    /// account for `fee_mint`; left unconstrained beyond "is a token
    /// account" because `transfer_checked` enforces the mint match
    /// itself, matching `close_market_treasury` and `sweep_residual`.
    ///
    /// Distinct from `rent_recipient` below, which receives the account's
    /// **lamports**: this one is a token account and takes the balance,
    /// that one is any address and takes the rent.
    #[account(mut)]
    pub token_recipient: InterfaceAccount<TokenAccount>,
    /// Receives the fee vault's rent lamports on close.
    /// CHECK: rent destination only.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseRegistryFeeVault {
    #[inline(always)]
    pub fn close_registry_fee_vault(&mut self) -> Result<()> {
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here.
        let bump_arr = [self.registry.bump];
        let registry_seed: &[u8] = b"registry";
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 2] = [registry_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        // Drain the collected fees to `token_recipient`, then close.
        // `transfer_out_leg` is the shared outbound payout helper, here
        // with the registry PDA as the signing authority — the same
        // zero-skip and `transfer_checked` shape, so a never-used fee
        // vault closes without a token CPI at all. No empty-account
        // assertion: `CloseAccount` below enforces that itself.
        let collected = self.fee_vault.amount();
        transfer_out_leg(
            self.token_program.address(),
            self.fee_vault.cpi_handle_mut(),
            self.fee_mint.cpi_handle(),
            self.token_recipient.cpi_handle_mut(),
            self.registry.cpi_handle(),
            collected,
            self.fee_mint.decimals(),
            &signer_seeds,
        )?;

        let cpi = CpiContext::new_with_signer(
            self.token_program.address(),
            CloseAccount {
                account: self.fee_vault.cpi_handle_mut(),
                destination: self.rent_recipient.cpi_handle_mut(),
                authority: self.registry.cpi_handle(),
            },
            &signer_seeds,
        );
        close_account(cpi)?;
        Ok(())
    }
}

// ── close_registry ────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct CloseRegistry {
    /// The last remaining registry admin — the only signer this accepts,
    /// and the one whose authority the close is performed under.
    pub admin: Signer,
    /// The registry being closed. `mut` so its lamports can be drained
    /// and its discriminator scrubbed by `Slab::close`.
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Receives the registry account's rent lamports on close.
    /// CHECK: rent destination only.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseRegistry {
    #[inline(always)]
    pub fn close_registry(&mut self) -> Result<()> {
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here. The caller must additionally be the *only*
        // admin, enforced by the `len() <= 1` check below.
        // Pre-condition: no live markets. `market_count` is the witness
        // that `close_market` ran for every market the registry created.
        require!(
            self.registry.market_count.get() == 0,
            DropsetError::RegistryHasMarkets
        );
        // Pre-condition: the admin set is down to the single caller.
        // `remove_admin` refuses to drop the last admin, so closing the
        // registry is the only path that removes it — and we only allow
        // it when exactly one admin (the signer) remains. The admin slab
        // tail length is the live admin count.
        require!(
            self.registry.len() <= 1,
            DropsetError::RegistryHasOtherAdmins
        );

        let dest = *self.rent_recipient.account();
        self.registry.close(dest)?;
        Ok(())
    }
}
