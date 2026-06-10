//! `withdraw_leader` — leader exit path.
//!
//! PDA-free variant of `withdraw` that burns `Vault.leader_shares`
//! directly. No `VaultDepositor` account is allocated or read. The
//! handler rejects when `signer != vault.leader`, so outside
//! depositors must use [`super::withdraw`] (the variant that loads
//! the depositor's basis PDA and crystallizes realized PnL).
//!
//! Active-vault min-leader-share floor is enforced here, matching the
//! spec § Vault → Skin-in-the-game floor.

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{RealizeEvent, WithdrawEvent},
    state::{realize_in_place, Market, PPM},
};

#[event_cpi]
#[derive(Accounts)]
pub struct WithdrawLeader {
    /// Must equal `vault.leader` — verified in-handler.
    #[account(mut)]
    pub signer: Signer,
    /// Market the vault lives on.
    #[account(mut)]
    pub market: Market,
    #[account(address = market.base_mint)]
    pub base_mint: InterfaceAccount<Mint>,
    #[account(address = market.quote_mint)]
    pub quote_mint: InterfaceAccount<Mint>,
    pub base_token_program: Interface<'static, TokenInterface>,
    pub quote_token_program: Interface<'static, TokenInterface>,
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = signer,
        associated_token::token_program = base_token_program,
    )]
    pub signer_base_ata: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = signer,
        associated_token::token_program = quote_token_program,
    )]
    pub signer_quote_ata: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = market,
        associated_token::token_program = base_token_program,
    )]
    pub market_base_treasury: InterfaceAccount<TokenAccount>,
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = market,
        associated_token::token_program = quote_token_program,
    )]
    pub market_quote_treasury: InterfaceAccount<TokenAccount>,
    pub associated_token_program: Program<AssociatedToken>,
    pub system_program: Program<System>,
}

impl WithdrawLeader {
    /// Returns `(Option<RealizeEvent>, WithdrawEvent)` for `lib.rs` to
    /// dispatch via `emit_cpi!`.
    #[inline(always)]
    pub fn withdraw_leader(
        &mut self,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<(Option<RealizeEvent>, WithdrawEvent)> {
        require!(shares_in > 0, DropsetError::InsufficientShares);
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let signer_addr = *self.signer.address();
        let (leader, frozen, total_shares, min_leader_share) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.frozen.get(),
                v.total_shares.get(),
                v.min_leader_share.get(),
            )
        };
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(total_shares > 0, DropsetError::InsufficientShares);
        require!(
            address_eq(&leader, &signer_addr),
            DropsetError::Unauthorized
        );

        // Realize first (per spec).
        let realize_outcome = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            realize_in_place(v)
        };
        let (total_shares, leader_shares, base_atoms, quote_atoms) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.total_shares.get(),
                v.leader_shares.get(),
                v.base_atoms.get(),
                v.quote_atoms.get(),
            )
        };

        // Pro-rata basket — floor.
        let ts = total_shares as u128;
        let s_in = shares_in as u128;
        let slice_base = (s_in * (base_atoms as u128)) / ts;
        let slice_quote = (s_in * (quote_atoms as u128)) / ts;
        require!(
            slice_base >= min_base_out as u128 && slice_quote >= min_quote_out as u128,
            DropsetError::BasketSlippage
        );
        let slice_base_u64 = slice_base as u64;
        let slice_quote_u64 = slice_quote as u64;

        require!(
            leader_shares >= shares_in,
            DropsetError::InsufficientShares
        );
        let new_leader = leader_shares - shares_in;
        // Skin-in-the-game floor on active vaults — bypassed for
        // frozen / tombstoned per spec.
        if !frozen {
            let new_total = total_shares - shares_in;
            if new_total > 0 {
                let lhs = (new_leader as u128) * (PPM as u128);
                let rhs = (min_leader_share as u128) * (new_total as u128);
                require!(lhs >= rhs, DropsetError::MinLeaderShareViolated);
            }
        }

        // Apply share + inventory burn.
        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let (new_total, new_base_atoms, new_quote_atoms) = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            let new_total = total_shares - shares_in;
            let new_base = base_atoms - slice_base_u64;
            let new_quote = quote_atoms - slice_quote_u64;
            v.total_shares = new_total.into();
            v.base_atoms = new_base.into();
            v.quote_atoms = new_quote.into();
            v.leader_shares = new_leader.into();
            (new_total, new_base, new_quote)
        };

        // CPI signer seeds for treasury → caller transfers.
        let bump_arr = [market_bump];
        let base_seed: &[u8] = base_mint_addr.as_ref();
        let quote_seed: &[u8] = quote_mint_addr.as_ref();
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 3] = [base_seed, quote_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        if slice_base_u64 > 0 {
            let decimals = self.base_mint.decimals();
            let cpi = CpiContext::new_with_signer(
                self.base_token_program.address(),
                TransferChecked {
                    from: self.market_base_treasury.cpi_handle_mut(),
                    mint: self.base_mint.cpi_handle(),
                    to: self.signer_base_ata.cpi_handle_mut(),
                    authority: self.market.cpi_handle(),
                },
                &signer_seeds,
            );
            transfer_checked(cpi, slice_base_u64, decimals)?;
        }
        if slice_quote_u64 > 0 {
            let decimals = self.quote_mint.decimals();
            let cpi = CpiContext::new_with_signer(
                self.quote_token_program.address(),
                TransferChecked {
                    from: self.market_quote_treasury.cpi_handle_mut(),
                    mint: self.quote_mint.cpi_handle(),
                    to: self.signer_quote_ata.cpi_handle_mut(),
                    authority: self.market.cpi_handle(),
                },
                &signer_seeds,
            );
            transfer_checked(cpi, slice_quote_u64, decimals)?;
        }

        let realize_event = if realize_outcome.shares_minted > 0 {
            Some(RealizeEvent {
                market: market_addr,
                sector_idx: vault_idx,
                shares_minted: realize_outcome.shares_minted,
                leader_shares_after: new_leader,
                total_shares_after: new_total,
                hwm_after: realize_outcome.hwm_after,
            })
        } else {
            None
        };
        let withdraw_event = WithdrawEvent {
            market: market_addr,
            sector_idx: vault_idx,
            depositor: signer_addr,
            is_leader: true,
            shares_in,
            base_out: slice_base_u64,
            quote_out: slice_quote_u64,
            total_shares_after: new_total,
            leader_shares_after: new_leader,
            base_atoms_after: new_base_atoms,
            quote_atoms_after: new_quote_atoms,
            realized_pnl_delta: 0,
        };
        Ok((realize_event, withdraw_event))
    }
}
