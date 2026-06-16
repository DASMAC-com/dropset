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
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{RealizeEvent, WithdrawEvent},
    state::{realize_in_place, Market, VaultDll},
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
    /// Either the vault's leader (burns `leader_shares`) or an outside
    /// depositor (burns the PDA's `shares`). PDA seeds bind the
    /// outside path to this signer.
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

        // Outside path: PDA seeds already bound to signer; just
        // verify shares balance and crystallize PnL. Leader-path
        // burns live in `withdraw_leader.rs`.
        let realized_pnl_delta: i64;
        // Snapshot the market address before the `vault_depositor`
        // mutable borrow so the defensive identity check below can
        // compare against it without re-borrowing `self.market`.
        let market_addr_check = *self.market.address();
        let new_leader_shares = {
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
            require!(
                vd.shares.get() >= shares_in,
                DropsetError::InsufficientShares
            );
            let released_basis = (vd.net_deposits.get() as u128)
                .checked_mul(s_in)
                .ok_or(DropsetError::MathOverflow)?
                / (vd.shares.get() as u128);
            // Realized PnL math, per spec L1513-1519:
            //   realized_fx    += slice_base × (ref_now − entry_ref)
            //   realized_yield += slice_quote + slice_base × entry_ref − released_basis
            //   realized_pnl   += slice_quote + slice_base × ref_now    − released_basis
            //
            // `ref_now × slice_base` and `entry_ref × slice_base` are
            // decoded via `Price::quote_for_base` — both produce a
            // quote-atom value, so the deltas are well-typed in
            // quote-denominated units. All math in `u128`/`i128` to
            // avoid intermediate overflow; the signed accumulators
            // clamp into `i64` at the store.
            let ref_now_price = crate::Price::from_bits(ref_price_bits);
            let entry_ref_price = vd.entry_ref_price;
            let quote_for_ref_now = ref_now_price
                .quote_for_base(slice_base_u64)
                .min(i128::MAX as u128) as i128;
            let quote_for_ref_entry = entry_ref_price
                .quote_for_base(slice_base_u64)
                .min(i128::MAX as u128) as i128;
            let slice_quote_i = slice_quote as i128;
            let released_i = released_basis as i128;
            let fx_delta: i128 = quote_for_ref_now.saturating_sub(quote_for_ref_entry);
            let yield_delta: i128 = slice_quote_i
                .saturating_add(quote_for_ref_entry)
                .saturating_sub(released_i);
            let pnl_delta: i128 = slice_quote_i
                .saturating_add(quote_for_ref_now)
                .saturating_sub(released_i);
            vd.realized_fx = (((vd.realized_fx.get() as i128).saturating_add(fx_delta))
                .clamp(i64::MIN as i128, i64::MAX as i128) as i64)
                .into();
            let new_pnl = (vd.realized_pnl.get() as i128).saturating_add(pnl_delta);
            let new_yield = (vd.realized_yield.get() as i128).saturating_add(yield_delta);
            vd.realized_pnl = (new_pnl.clamp(i64::MIN as i128, i64::MAX as i128) as i64).into();
            vd.realized_yield = (new_yield.clamp(i64::MIN as i128, i64::MAX as i128) as i64).into();
            realized_pnl_delta = pnl_delta.clamp(i64::MIN as i128, i64::MAX as i128) as i64;

            let new_shares = vd.shares.get() - shares_in;
            vd.shares = new_shares.into();
            vd.net_deposits = vd
                .net_deposits
                .get()
                .checked_sub(released_basis as u64)
                .ok_or(DropsetError::MathOverflow)?
                .into();
            // Counter decrement + PDA close happens after the transfer.
            leader_shares
        };

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

        // Transfer base + quote from treasuries → caller. `transfer_checked`
        // requires non-zero amounts on classic SPL Token; skip the CPI
        // when the leg is zero.
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

        // Close the VaultDepositor PDA when an outside depositor's
        // shares hit zero — refund rent to the depositor and
        // decrement `MarketHeader.outstanding_vault_depositors`. The
        // counter is the spec's only on-chain witness that
        // `close_market` can safely proceed (architecture.md §
        // Account lifecycle and rent reclamation), so it must come
        // back to zero on every clean exit.
        if self.vault_depositor.shares.get() == 0 {
            use anchor_lang_v2::AnchorAccount;
            let signer_view = *self.signer.as_ref();
            self.vault_depositor.close(signer_view)?;
            let prev = self.market.outstanding_vault_depositors.get();
            self.market.outstanding_vault_depositors = prev.saturating_sub(1).into();
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
