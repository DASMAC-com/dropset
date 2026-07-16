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
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use super::transfer_out_leg;
use crate::{
    errors::DropsetError,
    events::{RealizeEvent, WithdrawEvent},
    state::{
        compute_pro_rata_slice, min_leader_share_ok, realize_in_place, Market, VaultAccess,
        VaultDll,
    },
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

        let signer_addr = *self.signer.address();
        let (leader, frozen, tombstoned, total_shares, min_leader_share) = {
            let v = self.market.read_vault(vault_idx)?;
            require!(v.is_occupied(), DropsetError::VaultEmpty);
            (
                v.leader,
                v.frozen.get(),
                v.tombstoned.get(),
                v.total_shares.get(),
                v.min_leader_share.get(),
            )
        };
        require!(total_shares > 0, DropsetError::InsufficientShares);
        require!(
            address_eq(&leader, &signer_addr),
            DropsetError::Unauthorized
        );

        // Realize first (per spec).
        let realize_outcome = {
            let v = self.market.mutate_vault(vault_idx)?;
            realize_in_place(v)
        };
        let (total_shares, leader_shares, base_atoms, quote_atoms) = {
            let v = self.market.read_vault(vault_idx)?;
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
                require!(
                    min_leader_share_ok(new_leader, new_total, min_leader_share),
                    DropsetError::MinLeaderShareViolated
                );
            }
        }

        // Apply share + inventory burn.
        let market_addr = *self.market.address();
        let (new_total, new_base_atoms, new_quote_atoms) = {
            let v = self.market.mutate_vault(vault_idx)?;
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
        let (mint_seeds, bump_arr) = self.market.signer_seed_parts();
        let signer_seeds_inner: [&[u8]; 3] =
            [mint_seeds[0].as_ref(), mint_seeds[1].as_ref(), &bump_arr];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        transfer_out_leg(
            self.base_token_program.address(),
            self.market_base_treasury.cpi_handle_mut(),
            self.base_mint.cpi_handle(),
            self.signer_base_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_base_u64,
            self.base_mint.decimals(),
            &signer_seeds,
        )?;
        transfer_out_leg(
            self.quote_token_program.address(),
            self.market_quote_treasury.cpi_handle_mut(),
            self.quote_mint.cpi_handle(),
            self.signer_quote_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_quote_u64,
            self.quote_mint.decimals(),
            &signer_seeds,
        )?;

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

        let realize_event = RealizeEvent::from_outcome(
            &realize_outcome,
            market_addr,
            vault_idx,
            new_leader,
            new_total,
        );
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
