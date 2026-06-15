//! `deposit` — outside-depositor flow.
//!
//! Sizes by **one leg**; the matching leg is derived from the vault's
//! current ratio (`base_atoms`, `quote_atoms`) so VPS is preserved
//! (spec invariant I1). Calls [`realize_in_place`] first per spec —
//! outside flows always cross at the post-fee VPS. Allocates / tops
//! off a [`VaultDepositorHeader`] PDA that records cost basis
//! (`entry_vps`, `entry_ref_price`, `net_deposits`,
//! `gross_deposited`, `opened_at`) so a later `Withdraw` can
//! crystallize realized PnL.
//!
//! Seeding (`total_shares == 0`) is rejected here — vault seeding is
//! a leader-only operation that lives in
//! [`super::deposit_leader`]. The handler also rejects when
//! `signer == vault.leader` so the leader never allocates a
//! `VaultDepositor` PDA they won't use; the leader's top-up path is
//! `deposit_leader`.

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
    state::{isqrt_u128, realize_in_place, single_leg_basket, Market, PPM},
    VaultDepositorHeader,
};

#[event_cpi]
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
    /// `init_if_needed` so a first-time depositor pays rent inline
    /// and a top-off depositor sees the existing PDA. Seeds bind to
    /// `(market, sector_idx, signer)` so signer ownership is
    /// implicit — there is no separate `authority` field to
    /// reassign. The handler closes this PDA on zero-share exit
    /// (`withdraw`) so the depositor's rent comes back.
    #[account(
        init_if_needed,
        payer = signer,
        seeds = [
            b"vault_depositor",
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
    /// Returns `(Option<RealizeEvent>, DepositEvent)` for `lib.rs` to
    /// dispatch through `emit_cpi!`. See [`super::create_vault`] for
    /// the rationale on emitting outside the handler. `bump` is the
    /// `VaultDepositor` PDA bump from `ctx.bumps.vault_depositor` —
    /// stamped so `withdraw`'s `bump = vault_depositor.bump`
    /// reverification has a valid value to compare against.
    #[inline(always)]
    pub fn deposit(
        &mut self,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
        bump: u8,
    ) -> Result<(Option<RealizeEvent>, DepositEvent)> {
        // Stamp the canonical PDA bump so `withdraw`'s
        // `bump = vault_depositor.bump` reverification works.
        self.vault_depositor.bump = bump;

        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Snapshot pre-state we need post-mutation. Borrow the vault
        // immutably to read fields, drop the borrow, then re-borrow mut.
        let (
            leader,
            allow_outside,
            outside_approved,
            frozen,
            tombstoned,
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
                v.allow_outside_depositors.get(),
                v.outside_deposits_approved.get(),
                v.frozen.get(),
                v.tombstoned.get(),
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
        require!(!frozen, DropsetError::VaultFrozen);
        // A tombstoned vault is winding down — it no longer quotes and
        // accrues no fee, so minting fresh shares into it is rejected
        // (spec: deposits against frozen or tombstoned vaults are
        // rejected).
        require!(!tombstoned, DropsetError::VaultTombstoned);

        let signer_addr = *self.signer.address();
        // This handler is the outside-depositor path. The leader's
        // own deposits go through `deposit_leader` (no PDA, no basis
        // tracking). Reject the leader here so they don't allocate a
        // `VaultDepositor` PDA they'll never use.
        require!(
            !address_eq(&leader, &signer_addr),
            DropsetError::Unauthorized
        );

        // Seeding (`total_shares == 0`) requires the leader — and the
        // outside path is by definition not the leader. Reject up
        // front to give a clearer error than the share-math collapse.
        require!(total_shares > 0, DropsetError::SeedingRequiresLeader);

        require!(allow_outside, DropsetError::OutsideDepositorsNotAllowed);
        require!(outside_approved, DropsetError::OutsideDepositorsNotApproved);

        // The depositor's `entry_ref_price` is stamped here from
        // `vault.reference_price.price`; if the leader never set it
        // (still the zero sentinel) the basis math collapses
        // silently — `quote_for_base(ZERO, base) == 0`. Reject up
        // front rather than letting the depositor enter at a
        // nonsensical basis.
        require!(
            ref_price_bits != 0 && ref_price_bits != u32::MAX,
            DropsetError::ReferencePriceNotSet
        );

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

        // Compute the basket + shares_out. Seeding is rejected
        // earlier (this is the outside path), so `total_shares > 0`
        // by the time we get here — only the single-leg path runs.
        let (shares_out, base_in_final, quote_in_final) = single_leg_basket(
            total_shares,
            base_atoms,
            quote_atoms,
            base_in,
            quote_in,
            max_base_in,
            max_quote_in,
        )?;

        // Skin-in-the-game floor: post-deposit
        // `leader_shares / total_shares >= min_leader_share / PPM`.
        // Always enforced here — this handler is outside-only by
        // construction.
        {
            let new_total = (total_shares as u128) + (shares_out as u128);
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

        // Apply share + inventory mutations to the vault. Outside
        // path: `Vault.leader_shares` is unchanged; the depositor's
        // shares-out lands on the `VaultDepositor` PDA below.
        let market_addr = *self.market.address();
        let (new_total, new_leader_shares, new_base_atoms, new_quote_atoms) = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            v.base_atoms = (base_atoms + base_in_final).into();
            v.quote_atoms = (quote_atoms + quote_in_final).into();
            let new_total = total_shares + shares_out;
            v.total_shares = new_total.into();
            (
                new_total,
                leader_shares,
                v.base_atoms.get(),
                v.quote_atoms.get(),
            )
        };

        // Update the VaultDepositor basis fields. The first-deposit vs
        // top-off branching and the basis invariants live on
        // `VaultDepositorHeader::record_deposit`; the handler computes the
        // inputs and owns the `Market`-level counter bump.
        {
            // Post-deposit VPS = L / total_shares, Q32.32. Spec's
            // **Depositor positions and cost basis → Top-off** says
            // top-offs merge against `VPS_now` evaluated at the
            // post-deposit state.
            let l_after = isqrt_u128((new_base_atoms as u128) * (new_quote_atoms as u128));
            // `new_total > 0` always on this outside-only path: seeding is
            // rejected at `SeedingRequiresLeader` (so `total_shares > 0`)
            // and `single_leg_basket` guards `shares_out > 0`, hence
            // `new_total ≥ 2`. No zero-divisor guard needed here — the
            // seeding case it would cover is unreachable from `deposit`.
            let vps_after = ((l_after << 32) / new_total as u128) as u64;
            // Decoded reference price for cost-basis math.
            let ref_now_price = crate::Price::from_bits(ref_price_bits);
            // Quote-denominated lot value: `quote_in + base_in × ref`
            // (spec L944). Uses `quote_for_base` to decode the price.
            let lot_quote_value = (quote_in_final as u128)
                .saturating_add(ref_now_price.quote_for_base(base_in_final));
            let lot_quote_value_u64 = lot_quote_value.min(u64::MAX as u128) as u64;

            let is_first = self.vault_depositor.record_deposit(
                market_addr,
                vault_idx,
                signer_addr,
                shares_out,
                lot_quote_value_u64,
                vps_after,
                ref_now_price,
                self.clock.slot,
            );
            if is_first {
                // Fresh `VaultDepositor` PDA — bump the market's
                // outstanding depositor counter (Market state, not
                // depositor state).
                let prev = self.market.outstanding_vault_depositors.get();
                let next = prev.checked_add(1).ok_or(DropsetError::MathOverflow)?;
                self.market.outstanding_vault_depositors = next.into();
            }
        }

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
            is_leader: false,
            is_seeding: false,
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
