//! `close_registry_fee_vault` / `close_registry` — registry-side rent
//! reclamation, the final steps of the teardown / redeploy cycle.
//!
//! Both live behind the `admin-teardown` Cargo feature (see the
//! architecture spec's **Account lifecycle and rent reclamation**) and
//! are absent from the final immutable build.
//!
//! Order (once every market is closed): drain the fee vault via the
//! existing admin fee sweep → `close_registry_fee_vault` per fee ATA →
//! `remove_admin` down to the last admin → `close_registry`. After the
//! registry is closed the program holds zero on-chain state and the
//! upgrade authority can redeploy a fresh binary at the same id.

use anchor_lang_v2::{prelude::*, AnchorAccount};
#[allow(unused_imports)]
use anchor_spl_v2::{
    token_2022::{close_account, CloseAccount},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{errors::DropsetError, AdminSet, Registry};

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
    /// token_program)` by the constraint. Must be drained to zero (via
    /// the existing admin fee sweep) first.
    #[account(
        mut,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = token_program,
    )]
    pub fee_vault: InterfaceAccount<TokenAccount>,
    /// Receives the fee vault's rent lamports on close.
    /// CHECK: rent destination only.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseRegistryFeeVault {
    #[inline(always)]
    pub fn close_registry_fee_vault(&mut self) -> Result<()> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        require!(
            self.fee_vault.amount() == 0,
            DropsetError::TokenAccountNotEmpty
        );

        let bump_arr = [self.registry.bump];
        let registry_seed: &[u8] = b"registry";
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 2] = [registry_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];
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
        // The caller must be an admin — and, below, the *only* admin.
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
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
