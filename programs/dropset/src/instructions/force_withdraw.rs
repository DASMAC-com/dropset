//! `force_withdraw_depositor` / `force_withdraw_leader` — admin-driven
//! position drains for the teardown / redeploy cycle.
//!
//! Both live behind the `admin-teardown` Cargo feature (see the
//! architecture spec's **Account lifecycle and rent reclamation**) and
//! exist solely to make a market fully drainable without the
//! position-holder's signature, so the program can be upgrade-redeployed
//! against the same id between cycles. They are absent from the final
//! immutable build.
//!
//! Each is mechanically the same as its signed counterpart
//! ([`super::withdraw`] / [`super::withdraw_leader`]) for
//! `shares_in = <holder's full stake>`, with two differences per spec:
//!
//! * the signer gate is widened to `signer ∈ registry.admins`;
//! * funds always land with the position holder (the depositor `owner`
//!   or the vault `leader`), never the calling admin. If the holder's
//!   payout ATA is missing it is created via `init_if_needed`, and the
//!   admin running the wind-down bears that rent — a deliberate
//!   operational cost on the teardown wallet.
//!
//! The `min_leader_share` floor is **not** enforced here: the prescribed
//! teardown order drains every depositor first (step 1), so by the time
//! `force_withdraw_leader` runs `total_shares == leader_shares` and the
//! leader exits to zero. Draining the last leader share is exactly the
//! point of the instruction.

use anchor_lang_v2::{address_eq, prelude::*, AnchorAccount};
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
    AdminSet, Registry, VaultDepositorHeader,
};

// ── force_withdraw_depositor ──────────────────────────────────────────

#[event_cpi]
#[derive(Accounts)]
#[instruction(vault_idx: u32)]
pub struct ForceWithdrawDepositor {
    /// Registry admin. Funds the `owner` payout ATAs when
    /// `init_if_needed` has to allocate them.
    #[account(mut)]
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market the vault lives on.
    #[account(mut)]
    pub market: Market,
    /// The depositor whose position is being drained. Not a signer —
    /// that is the whole point of the admin force path. Used as the PDA
    /// seed and the payout-ATA authority so funds and rent both flow to
    /// the depositor, never the admin.
    /// `mut` because the closed `VaultDepositor` PDA refunds its rent
    /// here — the runtime rejects crediting lamports to a readonly account.
    /// CHECK: constrained by the `vault_depositor` PDA seeds (which bind
    /// this key) and the `associated_token::authority` constraints on
    /// the payout ATAs below.
    #[account(mut)]
    pub owner: UncheckedAccount,
    /// The depositor's basis PDA. Closed to `owner` on full drain.
    #[account(
        mut,
        seeds = [
            b"vault_depositor",
            market.address().as_ref(),
            &vault_idx.to_le_bytes(),
            owner.address().as_ref(),
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
    /// Depositor's base payout ATA — created (admin-funded) if absent.
    #[account(
        init_if_needed,
        payer = admin,
        associated_token::mint = base_mint,
        associated_token::authority = owner,
        associated_token::token_program = base_token_program,
    )]
    pub owner_base_ata: InterfaceAccount<TokenAccount>,
    /// Depositor's quote payout ATA — created (admin-funded) if absent.
    #[account(
        init_if_needed,
        payer = admin,
        associated_token::mint = quote_mint,
        associated_token::authority = owner,
        associated_token::token_program = quote_token_program,
    )]
    pub owner_quote_ata: InterfaceAccount<TokenAccount>,
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

impl ForceWithdrawDepositor {
    /// Drain the depositor's entire stake. Returns
    /// `(Option<RealizeEvent>, WithdrawEvent)` for `lib.rs` to dispatch.
    #[inline(always)]
    pub fn force_withdraw_depositor(
        &mut self,
        vault_idx: u32,
    ) -> Result<(Option<RealizeEvent>, WithdrawEvent)> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let owner_addr = *self.owner.address();
        let (leader, total_shares, ref_price_bits) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (
                v.leader,
                v.total_shares.get(),
                v.reference_price.price.as_u32(),
            )
        };
        // Reclaimed (free-list) sectors carry no depositors by the
        // teardown invariant — reject as defense-in-depth.
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(total_shares > 0, DropsetError::InsufficientShares);

        // Drain the depositor's whole position in one shot.
        let shares_in = self.vault_depositor.shares.get();
        require!(shares_in > 0, DropsetError::InsufficientShares);

        // Realize first (spec) — accrues any pending perf fee to the
        // leader before the basket is sliced.
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
        let slice_base_u64 = slice_base as u64;
        let slice_quote_u64 = slice_quote as u64;

        // Crystallize realized PnL into the depositor's accumulators —
        // identical math to the signed `withdraw` path (spec L1513-1519).
        let realized_pnl_delta: i64;
        let market_addr_check = *self.market.address();
        {
            let vd = &mut self.vault_depositor;
            require!(
                address_eq(&vd.market, &market_addr_check)
                    && vd.sector_idx.get() == vault_idx
                    && address_eq(&vd.owner, &owner_addr),
                DropsetError::VaultDepositorMismatch
            );
            let released_basis =
                ((vd.net_deposits.get() as u128) * s_in) / (vd.shares.get() as u128);
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
            vd.net_deposits = (vd.net_deposits.get() - (released_basis as u64)).into();
        }

        // Vault inventory + total share burn. `leader_shares` unchanged
        // (this is the depositor path).
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
            (new_total, new_base, new_quote)
        };

        // Market PDA signer seeds for the treasury → owner transfers.
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
                    to: self.owner_base_ata.cpi_handle_mut(),
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
                    to: self.owner_quote_ata.cpi_handle_mut(),
                    authority: self.market.cpi_handle(),
                },
                &signer_seeds,
            );
            transfer_checked(cpi, slice_quote_u64, decimals)?;
        }

        // Full drain by construction → close the PDA back to the
        // depositor and decrement the outstanding counter, exactly as
        // the signed close-on-empty path does.
        let owner_view = *self.owner.account();
        self.vault_depositor.close(owner_view)?;
        let prev = self.market.outstanding_vault_depositors.get();
        self.market.outstanding_vault_depositors = prev.saturating_sub(1).into();

        let realize_event = if realize_outcome.shares_minted > 0 {
            Some(RealizeEvent {
                market: market_addr,
                sector_idx: vault_idx,
                shares_minted: realize_outcome.shares_minted,
                leader_shares_after: leader_shares,
                total_shares_after: new_total,
                hwm_after: realize_outcome.hwm_after,
            })
        } else {
            None
        };
        let withdraw_event = WithdrawEvent {
            market: market_addr,
            sector_idx: vault_idx,
            depositor: owner_addr,
            is_leader: false,
            shares_in,
            base_out: slice_base_u64,
            quote_out: slice_quote_u64,
            total_shares_after: new_total,
            leader_shares_after: leader_shares,
            base_atoms_after: new_base_atoms,
            quote_atoms_after: new_quote_atoms,
            realized_pnl_delta,
        };
        Ok((realize_event, withdraw_event))
    }
}

// ── force_withdraw_leader ─────────────────────────────────────────────

#[event_cpi]
#[derive(Accounts)]
pub struct ForceWithdrawLeader {
    /// Registry admin. Funds the leader payout ATAs when `init_if_needed`
    /// has to allocate them.
    #[account(mut)]
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market the vault lives on.
    #[account(mut)]
    pub market: Market,
    /// The vault's leader — the payout target. Not a signer. Verified
    /// in-handler against `vault.leader`.
    /// CHECK: matched against `vault.leader` in the handler and bound as
    /// the `associated_token::authority` of the payout ATAs below.
    pub leader: UncheckedAccount,
    #[account(address = market.base_mint)]
    pub base_mint: InterfaceAccount<Mint>,
    #[account(address = market.quote_mint)]
    pub quote_mint: InterfaceAccount<Mint>,
    pub base_token_program: Interface<'static, TokenInterface>,
    pub quote_token_program: Interface<'static, TokenInterface>,
    /// Leader's base payout ATA — created (admin-funded) if absent.
    #[account(
        init_if_needed,
        payer = admin,
        associated_token::mint = base_mint,
        associated_token::authority = leader,
        associated_token::token_program = base_token_program,
    )]
    pub leader_base_ata: InterfaceAccount<TokenAccount>,
    /// Leader's quote payout ATA — created (admin-funded) if absent.
    #[account(
        init_if_needed,
        payer = admin,
        associated_token::mint = quote_mint,
        associated_token::authority = leader,
        associated_token::token_program = quote_token_program,
    )]
    pub leader_quote_ata: InterfaceAccount<TokenAccount>,
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

impl ForceWithdrawLeader {
    /// Drain the leader's entire `leader_shares` stake. Returns
    /// `(Option<RealizeEvent>, WithdrawEvent)` for `lib.rs` to dispatch.
    #[inline(always)]
    pub fn force_withdraw_leader(
        &mut self,
        vault_idx: u32,
    ) -> Result<(Option<RealizeEvent>, WithdrawEvent)> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let leader_addr = *self.leader.address();
        let (leader, total_shares) = {
            let v = &self.market.as_slice()[vault_idx as usize];
            (v.leader, v.total_shares.get())
        };
        // Reclaimed sectors have a zeroed leader — reject.
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        // The passed payout account must be the actual leader, so funds
        // can never be redirected to the calling admin.
        require!(
            address_eq(&leader, &leader_addr),
            DropsetError::Unauthorized
        );
        require!(total_shares > 0, DropsetError::InsufficientShares);

        // Realize first (spec). No-op on frozen vaults (HWM pinned).
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
        // Drain the leader's whole stake.
        let shares_in = leader_shares;
        require!(shares_in > 0, DropsetError::InsufficientShares);

        // Pro-rata basket — floor. The `min_leader_share` floor is
        // intentionally not enforced (see the module doc).
        let ts = total_shares as u128;
        let s_in = shares_in as u128;
        let slice_base = (s_in * (base_atoms as u128)) / ts;
        let slice_quote = (s_in * (quote_atoms as u128)) / ts;
        let slice_base_u64 = slice_base as u64;
        let slice_quote_u64 = slice_quote as u64;

        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let (new_total, new_leader, new_base_atoms, new_quote_atoms) = {
            let v = &mut self.market.as_mut_slice()[vault_idx as usize];
            let new_total = total_shares - shares_in;
            let new_leader = leader_shares - shares_in;
            let new_base = base_atoms - slice_base_u64;
            let new_quote = quote_atoms - slice_quote_u64;
            v.total_shares = new_total.into();
            v.leader_shares = new_leader.into();
            v.base_atoms = new_base.into();
            v.quote_atoms = new_quote.into();
            (new_total, new_leader, new_base, new_quote)
        };

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
                    to: self.leader_base_ata.cpi_handle_mut(),
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
                    to: self.leader_quote_ata.cpi_handle_mut(),
                    authority: self.market.cpi_handle(),
                },
                &signer_seeds,
            );
            transfer_checked(cpi, slice_quote_u64, decimals)?;
        }

        // Once the vault is fully drained, reclaim the sector to the
        // free DLL (spec's **Reclaim** step). This is an in-slab pointer
        // move — no rent is refunded; the sector's rent is part of the
        // market account, reclaimed wholesale by `close_market`.
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
            depositor: leader_addr,
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
