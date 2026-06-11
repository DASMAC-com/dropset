//! `close_market_treasury` / `close_market` — market-side rent
//! reclamation for the teardown / redeploy cycle.
//!
//! Both live behind the `admin-teardown` Cargo feature (see the
//! architecture spec's **Account lifecycle and rent reclamation**) and
//! are absent from the final immutable build.
//!
//! Teardown order (per market): drain every depositor
//! (`force_withdraw_depositor`) and every leader
//! (`force_withdraw_leader`) → `close_market_treasury` for each leg →
//! `close_market`. Each step's pre-condition is satisfied by the prior
//! one, so skipping ahead errors out rather than orphaning rent.

use anchor_lang_v2::{address_eq, prelude::*, AnchorAccount};
#[allow(unused_imports)]
use anchor_spl_v2::{
    token_2022::{close_account, CloseAccount},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{errors::DropsetError, state::Market, AdminSet, Registry};

// ── close_market_treasury ─────────────────────────────────────────────

#[derive(Accounts)]
pub struct CloseMarketTreasury {
    /// Registry admin — authorized via the registry admin set.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market owning the treasury. Read-only — closing a treasury does
    /// not mutate market state (the market is closed separately, after
    /// both treasuries are gone). The market PDA still signs the
    /// `CloseAccount` CPI via its `(base_mint, quote_mint)` seeds,
    /// recovered from `market.bump`. Taken bare (no `seeds` constraint),
    /// matching every other handler: the `associated_token::authority =
    /// market` constraint on `treasury` already binds the ATA to this
    /// market, and the CPI signature fails if a non-matching market is
    /// passed.
    #[account()]
    pub market: Market,
    /// One of the market's two leg mints. The ATA constraint below binds
    /// `treasury` to the canonical `(market, mint)` ATA; the handler
    /// additionally rejects any mint that isn't one of the market legs.
    pub mint: InterfaceAccount<Mint>,
    /// Token program owning `mint`.
    pub token_program: Interface<'static, TokenInterface>,
    /// The treasury ATA to close. The ATA constraint pins it to
    /// `ata(market, mint, token_program)`, so a non-canonical account is
    /// rejected before the handler runs. Must be drained to zero.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = market,
        associated_token::token_program = token_program,
    )]
    pub treasury: InterfaceAccount<TokenAccount>,
    /// Receives the treasury's rent lamports on close.
    /// CHECK: rent destination only; no constraints required — the admin
    /// chooses where reclaimed rent lands.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseMarketTreasury {
    #[inline(always)]
    pub fn close_market_treasury(&mut self) -> Result<()> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        // Defense-in-depth: a market only ever owns its two treasury
        // ATAs, but pin the mint to a real leg so a stray market-owned
        // ATA (none exist today) can't be closed by mistake.
        let mint_addr = *self.mint.address();
        require!(
            address_eq(&mint_addr, &self.market.base_mint)
                || address_eq(&mint_addr, &self.market.quote_mint),
            DropsetError::NotAMarketTreasury
        );
        // The treasury must be fully drained — its balance belongs to
        // the vaults' depositors/leaders, who are paid out first.
        require!(
            self.treasury.amount() == 0,
            DropsetError::TokenAccountNotEmpty
        );

        // Close the ATA, signed by the market PDA. Lamports flow to
        // `rent_recipient` inside the token program's `CloseAccount`.
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let bump_arr = [self.market.bump];
        let base_seed: &[u8] = base_mint_addr.as_ref();
        let quote_seed: &[u8] = quote_mint_addr.as_ref();
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 3] = [base_seed, quote_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];
        let cpi = CpiContext::new_with_signer(
            self.token_program.address(),
            CloseAccount {
                account: self.treasury.cpi_handle_mut(),
                destination: self.rent_recipient.cpi_handle_mut(),
                authority: self.market.cpi_handle(),
            },
            &signer_seeds,
        );
        close_account(cpi)?;
        Ok(())
    }
}

// ── close_market ──────────────────────────────────────────────────────

#[derive(Accounts)]
pub struct CloseMarket {
    /// Registry admin — authorized via the registry admin set.
    pub admin: Signer,
    /// Singleton registry. `mut` to decrement `market_count` — the
    /// witness `close_registry` later checks is zero.
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// The market being closed. `mut` so its lamports can be drained and
    /// its discriminator scrubbed by `Slab::close`. Taken bare, matching
    /// every other handler; the treasury `address` constraints below
    /// reference this market's stored treasury addresses, so a
    /// mismatched market is rejected before the close.
    #[account(mut)]
    pub market: Market,
    /// The base treasury — must already be closed (zero lamports).
    /// CHECK: pinned to `market.base_treasury` and required closed.
    #[account(address = market.base_treasury)]
    pub base_treasury: UncheckedAccount,
    /// The quote treasury — must already be closed (zero lamports).
    /// CHECK: pinned to `market.quote_treasury` and required closed.
    #[account(address = market.quote_treasury)]
    pub quote_treasury: UncheckedAccount,
    /// Receives the market account's rent lamports on close.
    /// CHECK: rent destination only.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseMarket {
    #[inline(always)]
    pub fn close_market(&mut self) -> Result<()> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        // Pre-condition 1: no outstanding depositor PDAs. This counter is
        // the only on-chain witness that no orphan `VaultDepositor` PDAs
        // remain (the program cannot enumerate all PDAs).
        require!(
            self.market.outstanding_vault_depositors.get() == 0,
            DropsetError::MarketHasDepositors
        );
        // Pre-condition 2: both treasuries already closed. A closed
        // account has been deallocated — zero lamports. Enforcing this
        // keeps `close_market` from orphaning treasury rent.
        require!(
            self.base_treasury.account().lamports() == 0
                && self.quote_treasury.account().lamports() == 0,
            DropsetError::MarketTreasuryNotClosed
        );

        // Decrement the live-market counter before closing — once the
        // market account is closed we can't read it again, and the
        // decrement must land on the registry regardless.
        let prev = self.registry.market_count.get();
        self.registry.market_count = prev.saturating_sub(1).into();

        // Reclaim the entire `MarketHeader` + vault slab in one shot.
        let dest = *self.rent_recipient.account();
        self.market.close(dest)?;
        Ok(())
    }
}
