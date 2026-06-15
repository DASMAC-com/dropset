//! `deposit_leader` — leader seeding + leader top-up flow.
//!
//! PDA-free variant of `deposit` that handles only the leader path:
//! either the vault's first deposit (`total_shares == 0`, both legs
//! supplied) or a subsequent leader top-up (single-leg sized). The
//! handler rejects when `signer != vault.leader`, so outside
//! depositors must use [`super::deposit`] (which carries the
//! `VaultDepositor` PDA).
//!
//! Avoiding the PDA on the leader path saves the ~0.0017 SOL rent
//! per allocation that the old combined `deposit` instruction
//! incurred (the leader's basis lives on `Vault.leader_shares`, not
//! on a per-depositor PDA — see spec § Shares).

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use super::transfer_in_leg;
use crate::{
    errors::DropsetError,
    events::{DepositEvent, RealizeEvent},
    state::{isqrt_u128, realize_in_place, single_leg_basket, Market},
    Q32_32_ONE,
};

#[event_cpi]
#[derive(Accounts)]
pub struct DepositLeader {
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
    pub system_program: Program<System>,
    pub associated_token_program: Program<AssociatedToken>,
}

impl DepositLeader {
    /// Returns `(Option<RealizeEvent>, DepositEvent)` for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn deposit_leader(
        &mut self,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<(Option<RealizeEvent>, DepositEvent)> {
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let signer_addr = *self.signer.address();
        let (leader, frozen, tombstoned, total_shares) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.frozen.get(),
                v.tombstoned.get(),
                v.total_shares.get(),
            )
        };
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(!frozen, DropsetError::VaultFrozen);
        // Tombstoned vaults are winding down: even the leader cannot
        // top up a closed vault (spec: deposits against frozen or
        // tombstoned vaults are rejected).
        require!(!tombstoned, DropsetError::VaultTombstoned);
        // The PDA-free path is strictly for the vault's leader. Any
        // other signer must use `deposit` (the outside variant).
        require!(
            address_eq(&leader, &signer_addr),
            DropsetError::Unauthorized
        );
        let is_seeding = total_shares == 0;

        // Realize first (no-op when seeding since total_shares == 0).
        let realize_outcome = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            realize_in_place(v)
        };
        let (total_shares, base_atoms, quote_atoms) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.total_shares.get(),
                v.base_atoms.get(),
                v.quote_atoms.get(),
            )
        };

        // Share / basket math — same as the outside path but without
        // the skin-in-the-game floor (leader path is exempt because
        // leader_shares strictly grows here).
        let (shares_out, base_in_final, quote_in_final) = if is_seeding {
            require!(
                base_in > 0 && quote_in > 0,
                DropsetError::SeedingRequiresBothLegs
            );
            let s = isqrt_u128((base_in as u128) * (quote_in as u128));
            require!(s > 0 && s <= u64::MAX as u128, DropsetError::MathOverflow);
            (s as u64, base_in, quote_in)
        } else {
            single_leg_basket(
                total_shares,
                base_atoms,
                quote_atoms,
                base_in,
                quote_in,
                max_base_in,
                max_quote_in,
            )?
        };

        // Transfer base + quote into the treasuries. `transfer_in_leg`
        // skips the CPI on a zero leg (`transfer_checked` rejects zero
        // amounts on classic SPL Token).
        transfer_in_leg(
            self.base_token_program.address(),
            self.signer_base_ata.cpi_handle_mut(),
            self.base_mint.cpi_handle(),
            self.market_base_treasury.cpi_handle_mut(),
            self.signer.cpi_handle(),
            base_in_final,
            self.base_mint.decimals(),
        )?;
        transfer_in_leg(
            self.quote_token_program.address(),
            self.signer_quote_ata.cpi_handle_mut(),
            self.quote_mint.cpi_handle(),
            self.market_quote_treasury.cpi_handle_mut(),
            self.signer.cpi_handle(),
            quote_in_final,
            self.quote_mint.decimals(),
        )?;

        // Apply vault mutations.
        let market_addr = *self.market.address();
        let (new_total, new_leader_shares, new_base_atoms, new_quote_atoms) = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            v.base_atoms = (base_atoms + base_in_final).into();
            v.quote_atoms = (quote_atoms + quote_in_final).into();
            let new_total = total_shares + shares_out;
            v.total_shares = new_total.into();
            let new_leader = v.leader_shares.get() + shares_out;
            v.leader_shares = new_leader.into();
            if is_seeding {
                v.hwm = Q32_32_ONE.into();
            }
            (
                new_total,
                new_leader,
                v.base_atoms.get(),
                v.quote_atoms.get(),
            )
        };

        let realize_event = if realize_outcome.shares_minted > 0 {
            Some(RealizeEvent {
                market: market_addr,
                sector_idx: vault_idx,
                shares_minted: realize_outcome.shares_minted,
                leader_shares_after: new_leader_shares,
                total_shares_after: new_total,
                hwm_after: realize_outcome.hwm_after,
            })
        } else {
            None
        };
        let deposit_event = DepositEvent {
            market: market_addr,
            sector_idx: vault_idx,
            depositor: signer_addr,
            is_leader: true,
            is_seeding,
            base_in: base_in_final,
            quote_in: quote_in_final,
            shares_out,
            total_shares_after: new_total,
            leader_shares_after: new_leader_shares,
            base_atoms_after: new_base_atoms,
            quote_atoms_after: new_quote_atoms,
        };
        Ok((realize_event, deposit_event))
    }
}
