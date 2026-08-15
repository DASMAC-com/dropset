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
//!
//! `close_market_treasury` drains before it closes rather than demanding
//! an already-empty ATA, because two balances legitimately outlive the
//! force-withdraw sweep and nothing else can move either one out: the
//! leg's `accrued_<leg>_fee_atoms` (protocol revenue, which
//! `sweep_residual` subtracts by design and no harvest instruction
//! exists to pay out) and any unsolicited transfer that landed after the
//! last sweep. Requiring zero would make both a permanent close-blocker
//! — the first for any market that ever charged a fee, the second as a
//! griefing vector, since anyone can send dust to a treasury ATA.
//!
//! The step's ordering guard survives that change, narrowed to the claim
//! it was really enforcing: the *vaults* must hold nothing for the leg
//! (`leg_vault_sum == 0`), so the force-withdraws still cannot be skipped
//! and depositor principal can never be routed to the drain recipient.

use anchor_lang_v2::{address_eq, prelude::*, AnchorAccount};
#[allow(unused_imports)]
use anchor_spl_v2::{
    token_2022::{close_account, CloseAccount},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{errors::DropsetError, state::Market, Registry};

use super::transfer_out_leg;
use crate::VaultAccess;

// ── close_market_treasury ─────────────────────────────────────────────

#[derive(Accounts)]
pub struct CloseMarketTreasury {
    /// Registry admin — authorized via the registry admin set.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market owning the treasury. `mut` so the leg's accrued-fee counter
    /// can be zeroed as its atoms leave — the counter and the balance it
    /// claims must move together, or the custody invariant reads as
    /// violated for the moment between (the market account is closed
    /// separately, after both treasuries are gone). The market PDA also
    /// signs both CPIs via its `(base_mint, quote_mint)` seeds, recovered
    /// from `market.bump`. Taken bare (no `seeds` constraint), matching
    /// every other handler: the `associated_token::authority = market`
    /// constraint on `treasury` already binds the ATA to this market, and
    /// the CPI signature fails if a non-matching market is passed.
    #[account(mut)]
    pub market: Market,
    /// One of the market's two leg mints. The ATA constraint below binds
    /// `treasury` to the canonical `(market, mint)` ATA; the handler
    /// additionally rejects any mint that isn't one of the market legs.
    pub mint: InterfaceAccount<Mint>,
    /// Token program owning `mint`.
    pub token_program: Interface<'static, TokenInterface>,
    /// The treasury ATA to close. The ATA constraint pins it to
    /// `ata(market, mint, token_program)`, so a non-canonical account is
    /// rejected before the handler runs. Drained to `token_recipient`
    /// below, then closed.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = market,
        associated_token::token_program = token_program,
    )]
    pub treasury: InterfaceAccount<TokenAccount>,
    /// Receives the treasury's remaining **tokens** — the accrued taker
    /// fee plus any unsolicited transfer — immediately before the close.
    /// Any admin-chosen token account for `mint`; left unconstrained
    /// beyond "is a token account" because `transfer_checked` enforces
    /// the mint match itself, matching `sweep_residual`'s destination.
    ///
    /// Distinct from `rent_recipient` below, which receives the account's
    /// **lamports**: this one is a token account and takes the balance,
    /// that one is any address and takes the rent.
    #[account(mut)]
    pub token_recipient: InterfaceAccount<TokenAccount>,
    /// Receives the treasury's rent lamports on close.
    /// CHECK: rent destination only; no constraints required — the admin
    /// chooses where reclaimed rent lands.
    #[account(mut)]
    pub rent_recipient: UncheckedAccount,
}

impl CloseMarketTreasury {
    #[inline(always)]
    pub fn close_market_treasury(&mut self) -> Result<()> {
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here.
        // Defense-in-depth: a market only ever owns its two treasury
        // ATAs, but pin the mint to a real leg so a stray market-owned
        // ATA (none exist today) can't be closed by mistake.
        let mint_addr = *self.mint.address();
        let is_base = address_eq(&mint_addr, &self.market.base_mint);
        require!(
            is_base || address_eq(&mint_addr, &self.market.quote_mint),
            DropsetError::NotAMarketTreasury
        );
        // Two pre-conditions, and together they are what make draining
        // safe (see this module's header for why draining at all is
        // necessary).
        //
        // First: no vault may still claim this leg, or the drain would
        // pay depositor principal to `token_recipient`.
        require!(
            self.market.leg_vault_sum(is_base) == 0,
            DropsetError::MarketVaultsNotDrained
        );
        // Second: the market must be at end of life. The claim check
        // alone does **not** imply that — a live market's leg sits at
        // zero whenever its vaults are one-sided, or once a vault has
        // been bought out of that leg entirely, both ordinary states. A
        // per-leg claim check on its own would therefore let an admin
        // harvest `accrued_<leg>_fee_atoms` from a *trading* market and
        // destroy the ATA under it, bricking the leg — nothing re-creates
        // a treasury for a market that already exists. An empty active
        // list is the witness that every sector has been reclaimed, which
        // the drain paths do on `total_shares == 0`.
        require!(
            self.market.active_count.get() == 0,
            DropsetError::MarketHasActiveVaults
        );

        let (mint_seeds, bump_arr) = self.market.signer_seed_parts();
        let signer_seeds_inner: [&[u8]; 3] =
            [mint_seeds[0].as_ref(), mint_seeds[1].as_ref(), &bump_arr];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        // Drain whatever is left to `token_recipient`, then close.
        // `transfer_out_leg` skips a zero amount, so the healthy zero-fee
        // teardown makes no CPI at all. No explicit empty-account check:
        // `CloseAccount` below refuses a non-empty account itself, so
        // re-asserting here would only duplicate the token program's own
        // guard.
        //
        // Read the balance up front — `cpi_handle_mut()` takes `treasury`
        // mutably for the duration of the call, so it can't also be read
        // in the argument list.
        let remainder = self.treasury.amount();
        transfer_out_leg(
            self.token_program.address(),
            self.treasury.cpi_handle_mut(),
            self.mint.cpi_handle(),
            self.token_recipient.cpi_handle_mut(),
            self.market.cpi_handle(),
            remainder,
            self.mint.decimals(),
            &signer_seeds,
        )?;
        // Zero the leg's counter now that its atoms are gone. Cosmetic in
        // isolation — `close_market` deallocates the account moments
        // later — but it keeps `treasury.amount >= Σ vault.<leg>_atoms +
        // accrued_<leg>_fee_atoms` literally true at every point in the
        // teardown, so the invariant needs no "except mid-close" caveat
        // and a partial teardown (base leg closed, quote still live)
        // can't be misread as revenue that was never paid out. Note the
        // `remainder` transferred just above is the whole balance — the
        // accrued fee *and* any unattributed residual (exact-in change, an
        // unsolicited transfer) that no `sweep_residual` collected first.
        if is_base {
            self.market.accrued_base_fee_atoms = 0u64.into();
        } else {
            self.market.accrued_quote_fee_atoms = 0u64.into();
        }

        // Close the ATA, signed by the market PDA. Lamports flow to
        // `rent_recipient` inside the token program's `CloseAccount`.
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
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here.
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
