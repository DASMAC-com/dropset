//! `sweep_residual` — admin-gated recovery of a market treasury's
//! **residual**: the atoms it holds that belong to neither the vaults'
//! depositors nor the protocol's accrued taker fee.
//!
//! ```txt
//! residual = treasury.amount − Σ vault.<leg>_atoms − accrued_<leg>_fee_atoms
//! ```
//!
//! In normal operation that value is exactly **zero** — the treasury
//! custody invariant (see the architecture spec's **MarketHeader → Fee
//! model**) says the two claimed terms sum to the balance. So a non-zero
//! residual is primarily a **bug alarm**, not income: it means a rounding
//! error, a share-math slip, or a botched rollback leaked or stranded
//! atoms, and an operator wants to see it. That tripwire is the reason the
//! protocol fee is tracked in explicit counters rather than *defined* as
//! the residual — under a residual definition any drift would be revenue
//! by construction, indistinguishable from depositor principal and
//! harvested away with no test able to fail.
//!
//! The legitimate non-zero case is an **unsolicited transfer**: anyone can
//! send tokens straight to the treasury ATA, and those atoms would
//! otherwise be stranded forever (no vault has a claim on them, so no
//! `Withdraw` can move them). This instruction recovers them.
//!
//! Deliberately **not** a fee harvest. The accrued counters are subtracted,
//! never touched — draining protocol revenue waits on a decided
//! destination (see the spec's **Fee model → Harvest**).

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::token_interface::{Mint, TokenAccount, TokenInterface};

use crate::{errors::DropsetError, events::SweepResidualEvent, state::Market, Registry};

use super::transfer_out_leg;
use crate::VaultAccess;

#[event_cpi]
#[derive(Accounts)]
pub struct SweepResidual {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market owning the treasury. Read-only: a sweep moves neither vault
    /// inventory nor the accrued-fee counters — it only pays out what
    /// neither of them claims. The market PDA still signs the transfer CPI
    /// via its `(base_mint, quote_mint)` seeds, recovered from
    /// `market.bump`. Taken bare (no `seeds` constraint), matching every
    /// other handler: the `associated_token::authority = market`
    /// constraint on `treasury` already binds the ATA to this market, and
    /// the CPI signature fails if a non-matching market is passed.
    #[account()]
    pub market: Market,
    /// The leg being swept. Must be one of the market's two mints; the
    /// handler rejects any other. It also selects the leg: which vault
    /// inventory field is summed, and which accrued counter is subtracted.
    pub mint: InterfaceAccount<Mint>,
    /// Token program owning `mint`.
    pub token_program: Interface<'static, TokenInterface>,
    /// The treasury ATA to sweep. Pinned to `ata(market, mint,
    /// token_program)`, so a non-canonical account is rejected before the
    /// handler runs.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = market,
        associated_token::token_program = token_program,
    )]
    pub treasury: InterfaceAccount<TokenAccount>,
    /// Where the residual lands — any admin-chosen token account for
    /// `mint`. Left unconstrained beyond "is a token account" on purpose:
    /// `transfer_checked` re-derives and enforces the mint match itself, so
    /// a `token::mint` constraint would only duplicate a check the CPI
    /// already runs, and the admin may legitimately want a non-ATA
    /// destination.
    #[account(mut)]
    pub destination: InterfaceAccount<TokenAccount>,
}

impl SweepResidual {
    /// Returns the [`SweepResidualEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`.
    #[inline(always)]
    pub fn sweep_residual(&mut self) -> Result<SweepResidualEvent> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        let mint_addr = *self.mint.address();
        let is_base = address_eq(&mint_addr, &self.market.base_mint);
        require!(
            is_base || address_eq(&mint_addr, &self.market.quote_mint),
            DropsetError::NotAMarketTreasury
        );

        // The depositors' and leaders' claim on this leg, summed across the
        // whole slab — see `leg_vault_sum` for why every sector counts.
        // Counting them all can only *understate* the residual here, which
        // errs toward leaving atoms in custody.
        let vault_sum = self.market.leg_vault_sum(is_base);
        let accrued = if is_base {
            self.market.accrued_base_fee_atoms.get()
        } else {
            self.market.accrued_quote_fee_atoms.get()
        };
        let treasury_amount = self.treasury.amount();
        // Saturating on purpose: a Token-2022 mint with a transfer-fee
        // extension delivers less than was sent, so the treasury can hold
        // *less* than the claimed sum and drive this negative. That is a
        // pre-existing threat to the custody invariant, not something this
        // instruction introduces; here it simply means there is nothing to
        // sweep. The emitted event carries all three terms so an operator
        // can see the shortfall.
        let claimed = vault_sum.saturating_add(accrued as u128);
        let residual = (treasury_amount as u128).saturating_sub(claimed);
        // Both `u128 → u64` conversions below clamp rather than wrap. This
        // one can't actually bind (`residual <= treasury_amount`, itself a
        // `u64`); the `vault_sum` one in the event payload can, if the slab
        // claims more atoms than a `u64` holds — a state that is already
        // corrupt, and where a literal `u64::MAX` in the read-out is itself
        // the alarm. Kept in the same shape so neither reads as the
        // considered case and the other as an oversight.
        let swept = residual.min(u64::MAX as u128) as u64;

        // A zero residual is the healthy case, and `transfer_out_leg`
        // skips a zero amount — so the instruction still succeeds and
        // still emits, which makes it usable as an on-chain read-out of
        // the invariant's three terms.
        let (mint_seeds, bump_arr) = self.market.signer_seed_parts();
        let signer_seeds_inner: [&[u8]; 3] =
            [mint_seeds[0].as_ref(), mint_seeds[1].as_ref(), &bump_arr];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];
        transfer_out_leg(
            self.token_program.address(),
            self.treasury.cpi_handle_mut(),
            self.mint.cpi_handle(),
            self.destination.cpi_handle_mut(),
            self.market.cpi_handle(),
            swept,
            self.mint.decimals(),
            &signer_seeds,
        )?;

        Ok(SweepResidualEvent {
            market: *self.market.address(),
            mint: mint_addr,
            destination: *self.destination.address(),
            treasury_amount,
            vault_sum: vault_sum.min(u64::MAX as u128) as u64,
            accrued_fee: accrued,
            swept,
        })
    }
}
