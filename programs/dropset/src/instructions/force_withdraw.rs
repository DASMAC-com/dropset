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
//!
//! A vault that was created but never seeded still occupies a sector, so
//! `active_count > 0` keeps `close_market` from running — yet it has no
//! stake to drain. `force_withdraw_leader` handles this empty case as a
//! no-op-then-reclaim: a zero-`total_shares` vault is returned straight to
//! the free list (`active_count--`) without a transfer, so admin teardown
//! can reclaim an empty vault rather than deadlocking on it.

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
    state::{compute_pro_rata_slice, realize_in_place, Market, VaultAccess, VaultDll},
    Registry, VaultDepositorHeader,
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
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here.
        let owner_addr = *self.owner.address();
        let (total_shares, ref_price_bits) = {
            let v = self.market.read_vault(vault_idx)?;
            // Reclaimed (free-list) sectors carry no depositors by the
            // teardown invariant — reject as defense-in-depth.
            require!(v.is_occupied(), DropsetError::VaultEmpty);
            (v.total_shares.get(), v.reference_price.price.as_u32())
        };
        require!(total_shares > 0, DropsetError::InsufficientShares);

        // Drain the depositor's whole position in one shot.
        let shares_in = self.vault_depositor.shares.get();
        require!(shares_in > 0, DropsetError::InsufficientShares);

        // Realize first (spec) — accrues any pending perf fee to the
        // leader before the basket is sliced.
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

        // Crystallize realized PnL + burn shares + reduce basis via the
        // shared helper — identical accounting to the signed `withdraw`
        // path.
        let market_addr_check = *self.market.address();
        let ref_now_price = crate::Price::from_bits(ref_price_bits);
        let realized_pnl_delta = {
            let vd = &mut self.vault_depositor;
            require!(
                address_eq(&vd.market, &market_addr_check)
                    && vd.sector_idx.get() == vault_idx
                    && address_eq(&vd.owner, &owner_addr),
                DropsetError::VaultDepositorMismatch
            );
            vd.crystallize_realized_pnl(shares_in, slice_base_u64, slice_quote_u64, ref_now_price)?
        };

        // Vault inventory + total share burn. `leader_shares` unchanged
        // (this is the depositor path).
        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let (new_total, new_base_atoms, new_quote_atoms) = {
            let v = self.market.mutate_vault(vault_idx)?;
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

        transfer_out_leg(
            self.base_token_program.address(),
            self.market_base_treasury.cpi_handle_mut(),
            self.base_mint.cpi_handle(),
            self.owner_base_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_base_u64,
            self.base_mint.decimals(),
            &signer_seeds,
        )?;
        transfer_out_leg(
            self.quote_token_program.address(),
            self.market_quote_treasury.cpi_handle_mut(),
            self.quote_mint.cpi_handle(),
            self.owner_quote_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_quote_u64,
            self.quote_mint.decimals(),
            &signer_seeds,
        )?;

        // Full drain by construction → close the PDA back to the
        // depositor via the shared helper (close + counter decrement),
        // exactly as the signed close-on-empty path does.
        let owner_view = *self.owner.account();
        close_depositor_and_decrement(&mut self.market, &mut self.vault_depositor, owner_view)?;

        // Mirror the leader path: if this depositor drain empties the
        // vault — possible when teardown runs out of the documented
        // order (leader already force-withdrawn, this the last
        // depositor) — reclaim the sector so the "a drained sector is
        // reclaimed" invariant holds regardless of drain order. The
        // sector is still threaded on active/tombstone here (the leader
        // path only reclaims on its own `new_total == 0`), so
        // `reclaim_sector`'s list pre-condition is satisfied.
        if new_total == 0 {
            self.market.reclaim_sector(vault_idx)?;
        }

        let realize_event = RealizeEvent::from_outcome(
            &realize_outcome,
            market_addr,
            vault_idx,
            leader_shares,
            new_total,
        );
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
        // Admin-only — gated at the dispatcher's feature-on arm via
        // `require_registry_admin` (`lib.rs`), so the caller is already a
        // known admin here.
        let leader_addr = *self.leader.address();
        let (leader, total_shares) = {
            let v = self.market.read_vault(vault_idx)?;
            // Reclaimed sectors have a zeroed leader — reject.
            require!(v.is_occupied(), DropsetError::VaultEmpty);
            (v.leader, v.total_shares.get())
        };
        // The passed payout account must be the actual leader, so funds
        // can never be redirected to the calling admin.
        require!(
            address_eq(&leader, &leader_addr),
            DropsetError::Unauthorized
        );

        // An empty (created-but-never-seeded) vault still occupies a
        // sector, so `active_count > 0` blocks `close_market` — yet there
        // is nothing to drain, and the share guards below would reject it
        // with `InsufficientShares`. Treat the zero-stake case as a
        // no-op-then-reclaim: return the sector to the free list
        // (`active_count--`) and exit with a zero-valued `WithdrawEvent`,
        // so admin teardown can reclaim an empty vault. `total_shares == 0`
        // implies no inventory, so no transfer or realize is needed.
        if total_shares == 0 {
            let market_addr = *self.market.address();
            self.market.reclaim_sector(vault_idx)?;
            let withdraw_event = WithdrawEvent {
                market: market_addr,
                sector_idx: vault_idx,
                depositor: leader_addr,
                is_leader: true,
                shares_in: 0,
                base_out: 0,
                quote_out: 0,
                total_shares_after: 0,
                leader_shares_after: 0,
                base_atoms_after: 0,
                quote_atoms_after: 0,
                realized_pnl_delta: 0,
            };
            return Ok((None, withdraw_event));
        }

        // Realize first (spec). No-op on frozen vaults (HWM pinned).
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
        // Drain the leader's whole stake.
        let shares_in = leader_shares;
        require!(shares_in > 0, DropsetError::InsufficientShares);

        // Pro-rata basket — floored slice, shared across withdraw paths.
        // The `min_leader_share` floor is intentionally not enforced (see
        // the module doc).
        let (slice_base_u64, slice_quote_u64) =
            compute_pro_rata_slice(shares_in, total_shares, base_atoms, quote_atoms);

        let market_addr = *self.market.address();
        let market_bump = self.market.bump;
        let base_mint_addr = self.market.base_mint;
        let quote_mint_addr = self.market.quote_mint;
        let (new_total, new_leader, new_base_atoms, new_quote_atoms) = {
            let v = self.market.mutate_vault(vault_idx)?;
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

        transfer_out_leg(
            self.base_token_program.address(),
            self.market_base_treasury.cpi_handle_mut(),
            self.base_mint.cpi_handle(),
            self.leader_base_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_base_u64,
            self.base_mint.decimals(),
            &signer_seeds,
        )?;
        transfer_out_leg(
            self.quote_token_program.address(),
            self.market_quote_treasury.cpi_handle_mut(),
            self.quote_mint.cpi_handle(),
            self.leader_quote_ata.cpi_handle_mut(),
            self.market.cpi_handle(),
            slice_quote_u64,
            self.quote_mint.decimals(),
            &signer_seeds,
        )?;

        // Once the vault is fully drained, reclaim the sector to the
        // free DLL (spec's **Reclaim** step). This is an in-slab pointer
        // move — no rent is refunded; the sector's rent is part of the
        // market account, reclaimed wholesale by `close_market`.
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
