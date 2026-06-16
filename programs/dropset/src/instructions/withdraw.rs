//! `withdraw` — outside-depositor exit path.
//!
//! Burns `shares_in` from the caller's `VaultDepositor.shares` (PDA
//! seeds bind to the signer, so authority is implicit). Computes the
//! floored pro-rata basket and transfers it from the market
//! treasuries to the caller's ATAs, signed by the market PDA.
//! Crystallizes realized PnL into the `VaultDepositor`'s
//! `realized_*` accumulators using
//! [`crate::Price::quote_for_base`] to decode `entry_ref_price` and
//! the live reference price, and reduces `net_deposits` by the
//! released basis slice. When the outside depositor's `shares`
//! reaches zero, the PDA is closed back to the depositor and
//! `MarketHeader.outstanding_vault_depositors` decremented.
//!
//! The leader's withdraw path lives in
//! [`super::withdraw_leader`] — this handler explicitly rejects
//! `signer == vault.leader` so the leader's signer never reaches
//! the PDA mutations or rent refund.

use anchor_lang_v2::{address_eq, prelude::*};
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use super::{close_depositor_and_decrement, transfer_out_leg};
use crate::{
    errors::DropsetError,
    events::{RealizeEvent, WithdrawEvent},
    state::{compute_pro_rata_slice, realize_in_place, Market, VaultDll},
    VaultDepositorHeader,
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(
    vault_idx: u32,
    shares_in: u64,
    min_base_out: u64,
    min_quote_out: u64,
)]
pub struct Withdraw {
    /// The outside depositor exiting the vault — the PDA seeds bind this
    /// signer to the `VaultDepositor` whose `shares` are burned here. The
    /// leader is rejected (`DropsetError::Unauthorized`) and exits via
    /// [`super::withdraw_leader`], which burns `leader_shares` directly
    /// and carries no `VaultDepositor` PDA.
    #[account(mut)]
    pub signer: Signer,
    /// Market the vault lives on.
    #[account(mut)]
    pub market: Market,
    /// Outside depositor's PDA. Mut so we can decrement `shares` and
    /// stamp realized PnL. The handler calls
    /// [`anchor_lang_v2::AnchorAccount::close`] explicitly when
    /// post-burn `shares == 0`, refunding the rent to the signer
    /// and decrementing `outstanding_vault_depositors` — a manual
    /// close keeps the conditional rent-refund logic in one place
    /// instead of relying on Anchor's unconditional `close = signer`
    /// attribute.
    #[account(
        mut,
        seeds = [
            b"vault_depositor",
            market.address().as_ref(),
            &vault_idx.to_le_bytes(),
            signer.address().as_ref(),
        ],
        bump = vault_depositor.bump,
    )]
    pub vault_depositor: Account<VaultDepositorHeader>,
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

impl Withdraw {
    /// Returns `(Option<RealizeEvent>, WithdrawEvent)` for `lib.rs` to
    /// dispatch via `emit_cpi!`.
    #[inline(always)]
    pub fn withdraw(
        &mut self,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<(Option<RealizeEvent>, WithdrawEvent)> {
        require!(shares_in > 0, DropsetError::InsufficientShares);
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Snapshot pre-state. `frozen` and `min_leader_share` were
        // needed by the deleted leader-path branch; the outside path
        // doesn't enforce the floor (the depositor isn't the leader)
        // and doesn't observe `frozen` (the deposit-side gate
        // already rejected outside flows on a frozen vault).
        let (leader, total_shares, ref_price_bits) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.total_shares.get(),
                v.reference_price.price.as_u32(),
            )
        };
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(total_shares > 0, DropsetError::InsufficientShares);

        let signer_addr = *self.signer.address();
        // Outside-depositor path only — the leader withdraws via
        // `withdraw_leader` (no PDA load). Rejecting here is what
        // makes the two-instruction split clean: the leader's
        // signer can never reach this handler's `VaultDepositor`
        // mutations or the rent refund on zero-share close.
        require!(
            !address_eq(&leader, &signer_addr),
            DropsetError::Unauthorized
        );

        // Realize first (spec).
        let realize_outcome = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            realize_in_place(v)
        };
        // Re-read after realize — `total_shares` may have grown if the
        // leader minted perf-fee shares.
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

        // Outside path: PDA seeds already bound to signer; verify the
        // stored identity, then crystallize realized PnL + burn the
        // shares + reduce basis via the shared helper. Leader-path burns
        // live in `withdraw_leader.rs`.
        // Snapshot the market address before the `vault_depositor`
        // mutable borrow so the defensive identity check can compare
        // against it without re-borrowing `self.market`.
        let market_addr_check = *self.market.address();
        let ref_now_price = crate::Price::from_bits(ref_price_bits);
        let realized_pnl_delta = {
            let vd = &mut self.vault_depositor;
            // Defensive: the PDA seeds (market, vault_idx, signer)
            // already bind this account, so its stored identity fields
            // must agree. Assert it explicitly — a future change to the
            // seed derivation, or an account reconstructed by other
            // means, is caught here instead of silently crediting the
            // wrong position's realized PnL / share burn.
            require!(
                address_eq(&vd.market, &market_addr_check)
                    && vd.sector_idx.get() == vault_idx
                    && address_eq(&vd.owner, &signer_addr),
                DropsetError::VaultDepositorMismatch
            );
            // Counter decrement + PDA close happens after the transfer.
            vd.crystallize_realized_pnl(shares_in, slice_base_u64, slice_quote_u64, ref_now_price)?
        };
        // Outside path: `leader_shares` is unchanged. The leader-only
        // branch lives in `withdraw_leader.rs`.
        let new_leader_shares = leader_shares;

        // Vault inventory + total share burn.
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
            // Outside path: leader_shares is unchanged. The
            // leader-only branch lives in `withdraw_leader.rs`.
            (new_total, new_base, new_quote)
        };

        // Build the market PDA signer seeds — used by both CPI transfers.
        let bump_arr = [market_bump];
        let base_seed: &[u8] = base_mint_addr.as_ref();
        let quote_seed: &[u8] = quote_mint_addr.as_ref();
        let bump_seed: &[u8] = &bump_arr;
        let signer_seeds_inner: [&[u8]; 3] = [base_seed, quote_seed, bump_seed];
        let signer_seeds: [&[&[u8]]; 1] = [&signer_seeds_inner];

        // Transfer base + quote from treasuries → caller via the shared
        // outbound helper (skips the CPI on a zero leg).
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

        // Close the VaultDepositor PDA when an outside depositor's
        // shares hit zero, refunding rent to the depositor. The shared
        // helper keeps the close and the `outstanding_vault_depositors`
        // decrement together (see its doc).
        if self.vault_depositor.shares.get() == 0 {
            let signer_view = *self.signer.as_ref();
            close_depositor_and_decrement(
                &mut self.market,
                &mut self.vault_depositor,
                signer_view,
            )?;
        }

        // If this outside withdraw drained the sector's last share,
        // return it to the free list. Reachable on a winding-down vault:
        // the leader has already exited to zero (the `withdraw_leader`
        // floor is bypassed once frozen/tombstoned), so the last outside
        // depositor's exit zeroes `total_shares`. Sequenced after the
        // `VaultDepositor` close so the two `self.market` borrows don't
        // overlap. Without this the drained sector leaks: it stays
        // threaded on the active/tombstone DLL with a non-default
        // `leader`, the slab slot is never recycled by `CreateVault`'s
        // free-list pop, and the `active_count` it holds against
        // `max_vaults_per_market` is never decremented — both freed only
        // inside `reclaim_sector`. Mirrors `force_withdraw.rs` (spec §
        // Withdraw and § Storage layout).
        if new_total == 0 {
            self.market.reclaim_sector(vault_idx)?;
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
        let withdraw_event = WithdrawEvent {
            market: market_addr,
            sector_idx: vault_idx,
            depositor: signer_addr,
            is_leader: false,
            shares_in,
            base_out: slice_base_u64,
            quote_out: slice_quote_u64,
            total_shares_after: new_total,
            leader_shares_after: new_leader_shares,
            base_atoms_after: new_base_atoms,
            quote_atoms_after: new_quote_atoms,
            realized_pnl_delta,
        };
        Ok((realize_event, withdraw_event))
    }
}
