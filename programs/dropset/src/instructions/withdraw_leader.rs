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
    state::{compute_pro_rata_slice, realize_in_place, Market, VaultDll, PPM},
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
        let (leader, frozen, tombstoned, total_shares, min_leader_share) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.frozen.get(),
                v.tombstoned.get(),
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

        // Pro-rata basket — floored slice, shared across withdraw paths.
        let (slice_base_u64, slice_quote_u64) =
            compute_pro_rata_slice(shares_in, total_shares, base_atoms, quote_atoms);
        require!(
            slice_base_u64 >= min_base_out && slice_quote_u64 >= min_quote_out,
            DropsetError::BasketSlippage
        );

        require!(leader_shares >= shares_in, DropsetError::InsufficientShares);
        let new_leader = leader_shares - shares_in;
        // Skin-in-the-game floor on active vaults — bypassed once the
        // vault is winding down (frozen or tombstoned) per spec, so the
        // leader can drain their final stake on the exit path.
        let winding_down = frozen || tombstoned;
        if !winding_down {
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

        // Reclaim the sector when the leader's exit drains it to zero.
        // Reachable when the leader is the sole holder (no outside
        // depositors) and burns their full stake — the min-leader-share
        // floor only guards `new_total > 0`, so a full drain is allowed
        // on active and winding-down vaults alike. Leaving it unreclaimed
        // leaks the slab slot and its `active_count` reservation, both
        // released only inside `reclaim_sector`. Mirrors
        // `force_withdraw.rs` (spec § Withdraw and § Storage layout).
        if new_total == 0 {
            self.market.reclaim_sector(vault_idx)?;
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
