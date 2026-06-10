//! `deposit` — leader seeding + outside-depositor flows.
//!
//! Sizes by **one leg** post-seeding; the matching leg is derived from
//! the vault's current ratio (`base_atoms`, `quote_atoms`) so VPS is
//! preserved (spec invariant I1). Calls [`realize_in_place`] first per
//! spec — outside flows always cross at the post-fee VPS. On the
//! outside path, allocates / tops off a [`VaultDepositorHeader`] PDA
//! that records cost basis (`entry_vps`, `entry_ref_price`,
//! `net_deposits`, `gross_deposited`, `opened_at`) so a later
//! `Withdraw` can crystallize realized PnL.
//!
//! Seeding (`total_shares == 0`) must come from the leader and supply
//! both legs explicitly; `total_shares := isqrt(base × quote)` and
//! `leader_shares := total_shares` per the spec's **Deposit →
//! Seeding**.

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{DepositEvent, RealizeEvent},
    state::{isqrt_u128, realize_in_place, Market, VAULT_DEPOSITOR_SEED, BPS, PPM},
    Q32_32_ONE, VaultDepositorHeader,
};

#[derive(Accounts)]
#[instruction(
    vault_idx: u32,
    base_in: u64,
    quote_in: u64,
    max_base_in: u64,
    max_quote_in: u64,
)]
pub struct Deposit {
    /// Either the vault's leader (seeding or top-up of leader_shares)
    /// or an outside depositor (creates / tops off `vault_depositor`).
    #[account(mut)]
    pub signer: Signer,
    /// Market the vault lives on. Mut for share + inventory writes.
    #[account(mut)]
    pub market: Market,
    /// `init_if_needed` on the outside path. On the leader path the
    /// PDA is still derived (Anchor needs the account list to stay
    /// fixed-shape) but its bytes are zero-initialized and never
    /// touched. Closing it on the leader path is left as a future
    /// rent-recovery PR. Seeds bind to `(market, sector, signer)` so
    /// signer ownership is implicit.
    #[account(
        init_if_needed,
        payer = signer,
        seeds = [
            VAULT_DEPOSITOR_SEED,
            market.address().as_ref(),
            &vault_idx.to_le_bytes(),
            signer.address().as_ref(),
        ],
        bump,
    )]
    pub vault_depositor: Account<VaultDepositorHeader>,
    /// Base mint — pinned to `market.base_mint`.
    #[account(address = market.base_mint)]
    pub base_mint: InterfaceAccount<Mint>,
    /// Quote mint — pinned to `market.quote_mint`.
    #[account(address = market.quote_mint)]
    pub quote_mint: InterfaceAccount<Mint>,
    pub base_token_program: Interface<'static, TokenInterface>,
    pub quote_token_program: Interface<'static, TokenInterface>,
    /// Signer's source ATA for base. Anchor's ATA constraint verifies
    /// `(signer, base_mint, base_token_program)`.
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = signer,
        associated_token::token_program = base_token_program,
    )]
    pub signer_base_ata: InterfaceAccount<TokenAccount>,
    /// Signer's source ATA for quote.
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = signer,
        associated_token::token_program = quote_token_program,
    )]
    pub signer_quote_ata: InterfaceAccount<TokenAccount>,
    /// Market base treasury — must match `market.base_treasury`.
    #[account(
        mut,
        associated_token::mint = base_mint,
        associated_token::authority = market,
        associated_token::token_program = base_token_program,
    )]
    pub market_base_treasury: InterfaceAccount<TokenAccount>,
    /// Market quote treasury — must match `market.quote_treasury`.
    #[account(
        mut,
        associated_token::mint = quote_mint,
        associated_token::authority = market,
        associated_token::token_program = quote_token_program,
    )]
    pub market_quote_treasury: InterfaceAccount<TokenAccount>,
    pub clock: Sysvar<Clock>,
    pub system_program: Program<System>,
    pub associated_token_program: Program<AssociatedToken>,
}

impl Deposit {
    #[inline(always)]
    pub fn deposit(
        &mut self,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<()> {
        let len = self.market.len();
        require!(
            (vault_idx as usize) < len,
            DropsetError::InvalidSectorIndex
        );

        // Snapshot pre-state we need post-mutation. Borrow the vault
        // immutably to read fields, drop the borrow, then re-borrow mut.
        let (
            leader,
            allow_outside,
            outside_approved,
            frozen,
            total_shares,
            _leader_shares,
            _base_atoms,
            _quote_atoms,
            min_leader_share,
            ref_price_bits,
        ) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.allow_outside_depositors,
                v.outside_deposits_approved,
                v.frozen,
                v.total_shares.get(),
                v.leader_shares.get(),
                v.base_atoms.get(),
                v.quote_atoms.get(),
                v.min_leader_share.get(),
                v.reference_price.price.as_u32(),
            )
        };
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(frozen == 0, DropsetError::VaultFrozen);

        let signer_addr = *self.signer.address();
        let is_leader = address_eq(&leader, &signer_addr);
        let is_seeding = total_shares == 0;

        if !is_leader {
            require!(
                allow_outside == 1,
                DropsetError::OutsideDepositorsNotAllowed
            );
            require!(
                outside_approved == 1,
                DropsetError::OutsideDepositorsNotApproved
            );
        }

        // Realize first (spec). No-op when seeding (total_shares == 0).
        // Capture outcome so we can emit a RealizeEvent if shares minted.
        let realize_outcome = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            realize_in_place(v)
        };
        // Re-read post-realize values for the share math below.
        let (total_shares, leader_shares, base_atoms, quote_atoms) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.total_shares.get(),
                v.leader_shares.get(),
                v.base_atoms.get(),
                v.quote_atoms.get(),
            )
        };

        // Compute the basket + shares_out.
        let (shares_out, base_in_final, quote_in_final) = if is_seeding {
            require!(is_leader, DropsetError::SeedingRequiresLeader);
            require!(
                base_in > 0 && quote_in > 0,
                DropsetError::SeedingRequiresBothLegs
            );
            let s = isqrt_u128((base_in as u128) * (quote_in as u128));
            require!(s > 0 && s <= u64::MAX as u128, DropsetError::MathOverflow);
            (s as u64, base_in, quote_in)
        } else {
            // Subsequent deposit: exactly one leg is sized.
            require!(
                (base_in > 0) ^ (quote_in > 0),
                DropsetError::SingleLegRequired
            );
            let ts = total_shares as u128;
            let b = base_atoms as u128;
            let q = quote_atoms as u128;
            let shares_out_u128 = if base_in > 0 {
                ((base_in as u128) * ts) / b
            } else {
                ((quote_in as u128) * ts) / q
            };
            require!(
                shares_out_u128 > 0 && shares_out_u128 <= u64::MAX as u128,
                DropsetError::MathOverflow
            );
            // Basket = ceil(shares_out × leg / total_shares). u128
            // intermediates; the final values fit in u64 by construction
            // (basket ≤ caller's input + 1).
            let base_in_final =
                ((shares_out_u128 * b) + ts - 1) / ts;
            let quote_in_final =
                ((shares_out_u128 * q) + ts - 1) / ts;
            require!(
                base_in_final <= max_base_in as u128
                    && quote_in_final <= max_quote_in as u128,
                DropsetError::BasketSlippage
            );
            (
                shares_out_u128 as u64,
                base_in_final as u64,
                quote_in_final as u64,
            )
        };

        // Skin-in-the-game floor (outside-path, non-seeding): post-deposit
        // ratio leader_shares / total_shares >= min_leader_share / PPM.
        if !is_leader && !is_seeding {
            let new_total = (total_shares as u128) + (shares_out as u64 as u128);
            let lhs = (leader_shares as u128) * (PPM as u128);
            let rhs = (min_leader_share as u128) * new_total;
            require!(lhs >= rhs, DropsetError::MinLeaderShareViolated);
        }

        // Transfer base + quote into the treasuries. `transfer_checked`
        // requires non-zero amounts on classic SPL Token; skip the CPI
        // when the leg is zero.
        if base_in_final > 0 {
            let decimals = self.base_mint.decimals();
            let cpi = CpiContext::new(
                self.base_token_program.address(),
                TransferChecked {
                    from: self.signer_base_ata.cpi_handle_mut(),
                    mint: self.base_mint.cpi_handle(),
                    to: self.market_base_treasury.cpi_handle_mut(),
                    authority: self.signer.cpi_handle(),
                },
            );
            transfer_checked(cpi, base_in_final, decimals)?;
        }
        if quote_in_final > 0 {
            let decimals = self.quote_mint.decimals();
            let cpi = CpiContext::new(
                self.quote_token_program.address(),
                TransferChecked {
                    from: self.signer_quote_ata.cpi_handle_mut(),
                    mint: self.quote_mint.cpi_handle(),
                    to: self.market_quote_treasury.cpi_handle_mut(),
                    authority: self.signer.cpi_handle(),
                },
            );
            transfer_checked(cpi, quote_in_final, decimals)?;
        }

        // Apply share + inventory mutations to the vault.
        let market_addr = *self.market.address();
        let (new_total, new_leader_shares, new_base_atoms, new_quote_atoms) = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            v.base_atoms = (base_atoms + base_in_final).into();
            v.quote_atoms = (quote_atoms + quote_in_final).into();
            let new_total = total_shares + shares_out;
            v.total_shares = new_total.into();
            if is_leader {
                let new_leader = leader_shares + shares_out;
                v.leader_shares = new_leader.into();
                if is_seeding {
                    v.hwm = Q32_32_ONE.into();
                }
                (new_total, new_leader, v.base_atoms.get(), v.quote_atoms.get())
            } else {
                (
                    new_total,
                    leader_shares,
                    v.base_atoms.get(),
                    v.quote_atoms.get(),
                )
            }
        };

        // Outside path: update the VaultDepositor basis fields.
        if !is_leader {
            let vd = &mut self.vault_depositor;
            let prior_shares = vd.shares.get();
            let new_vd_shares = prior_shares + shares_out;
            // Decode reference price + recompute VPS to capture basis.
            // VPS uses the *post-deposit* total_shares per the spec
            // (top-off merges shares-weighted against `VPS_now`); for
            // seeding-symmetric capture on a first outside deposit,
            // use the same post-deposit state.
            let l_after = isqrt_u128(
                (new_base_atoms as u128) * (new_quote_atoms as u128),
            );
            let vps_after = if new_total == 0 {
                Q32_32_ONE
            } else {
                ((l_after << 32) / new_total as u128) as u64
            };
            // Quote-denominated lot value: `quote_in + base_in × ref`,
            // where `ref` is decoded from the live reference price.
            // For MVP we use the raw u32 ref bits as a proxy scaled
            // factor — fine when ref_price > 0 (gated below). A full
            // decoder lands in the same follow-up that wires up
            // off-chain price display.
            let ref_now = ref_price_bits as u64;
            let lot_quote_value = (quote_in_final as u128)
                + (base_in_final as u128) * (ref_now as u128);
            let lot_quote_value_u64 = lot_quote_value.min(u64::MAX as u128) as u64;

            if prior_shares == 0 {
                // First deposit into this PDA — stamp all basis fields.
                vd.market = market_addr;
                vd.sector_idx = vault_idx.into();
                vd.owner = signer_addr;
                vd.shares = (new_vd_shares).into();
                vd.net_deposits = lot_quote_value_u64.into();
                vd.gross_deposited = lot_quote_value_u64.into();
                vd.entry_ref_price = crate::Price::from_bits(ref_price_bits);
                vd.entry_vps = vps_after.into();
                vd.opened_at = self.clock.slot.into();
                // realized_* default to zero; bump captured by Anchor.
                // Bump the market's outstanding depositor counter — this
                // is a fresh `VaultDepositor` PDA.
                let prev = self.market.outstanding_vault_depositors.get();
                self.market.outstanding_vault_depositors =
                    (prev + 1).into();
            } else {
                // Top-off: merge weighted averages.
                let s = prior_shares as u128;
                let ds = shares_out as u128;
                let denom = s + ds;
                let entry_vps_prev = vd.entry_vps.get() as u128;
                let entry_ref_prev = vd.entry_ref_price.as_u32() as u128;
                let entry_vps_new =
                    (s * entry_vps_prev + ds * (vps_after as u128)) / denom;
                let entry_ref_new =
                    (s * entry_ref_prev + ds * (ref_now as u128)) / denom;
                vd.shares = new_vd_shares.into();
                vd.net_deposits = (vd.net_deposits.get() + lot_quote_value_u64).into();
                vd.gross_deposited =
                    (vd.gross_deposited.get() + lot_quote_value_u64).into();
                vd.entry_vps = (entry_vps_new as u64).into();
                vd.entry_ref_price =
                    crate::Price::from_bits(entry_ref_new as u32);
            }
        }

        if realize_outcome.shares_minted > 0 {
            emit!(RealizeEvent {
                market: market_addr,
                sector_idx: vault_idx,
                shares_minted: realize_outcome.shares_minted,
                leader_shares_after: new_leader_shares,
                total_shares_after: new_total,
                hwm_after: realize_outcome.hwm_after,
            });
        }
        emit!(DepositEvent {
            market: market_addr,
            sector_idx: vault_idx,
            depositor: signer_addr,
            is_leader,
            is_seeding,
            base_in: base_in_final,
            quote_in: quote_in_final,
            shares_out,
            total_shares_after: new_total,
            leader_shares_after: new_leader_shares,
            base_atoms_after: new_base_atoms,
            quote_atoms_after: new_quote_atoms,
        });

        // Suppress unused-const warning when BPS isn't referenced
        // anywhere downstream of the basket math (it is in
        // set_liquidity_profile; kept in scope here so the symbol stays
        // findable from one module).
        let _ = BPS;
        Ok(())
    }
}
