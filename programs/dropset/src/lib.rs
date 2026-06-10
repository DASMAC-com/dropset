// cspell:word discrim
use anchor_lang_v2::prelude::*;

mod errors;
mod events;
mod instructions;
mod price;
mod state;

pub use errors::*;
pub use events::*;
use instructions::*;
pub use price::*;
pub use state::*;

declare_id!("TESTnXwv2eHoftsSd5NEdpH4zEu7XRC8jviuoNPdB2Q");

#[program]
pub mod dropset {
    use super::*;

    #[discrim = 0]
    #[access_control(ctx.accounts.verify_upgrade_authority(ctx.program_id))]
    pub fn init(ctx: &mut Context<Init>, genesis_admin: Address, fee_atoms: u64) -> Result<()> {
        ctx.accounts
            .init(ctx.bumps.registry, genesis_admin, fee_atoms)
    }

    #[discrim = 1]
    pub fn add_admin(ctx: &mut Context<AddAdmin>, new_admin: Address) -> Result<()> {
        ctx.accounts.add_admin(new_admin)
    }

    #[discrim = 2]
    pub fn remove_admin(ctx: &mut Context<RemoveAdmin>, target: Address) -> Result<()> {
        ctx.accounts.remove_admin(target)
    }

    #[discrim = 3]
    pub fn register_market(ctx: &mut Context<RegisterMarket>) -> Result<()> {
        ctx.accounts.register_market(ctx.bumps.market)
    }

    #[discrim = 4]
    pub fn register_vault(
        ctx: &mut Context<RegisterVault>,
        perf_fee_rate: u32,
        quote_authority: Address,
        allow_outside_depositors: bool,
        leader_override: Address,
    ) -> Result<()> {
        let event = ctx.accounts.register_vault(
            perf_fee_rate,
            quote_authority,
            allow_outside_depositors,
            leader_override,
        )?;
        emit_cpi!(event);
        Ok(())
    }

    #[discrim = 5]
    pub fn set_reference_price(
        ctx: &mut Context<SetReferencePrice>,
        vault_idx: u32,
        price_bits: u32,
        quote_slot: u64,
    ) -> Result<()> {
        ctx.accounts
            .set_reference_price(vault_idx, Price::from_bits(price_bits), quote_slot)
    }

    #[discrim = 6]
    pub fn set_liquidity_profile(
        ctx: &mut Context<SetLiquidityProfile>,
        vault_idx: u32,
        profile_bytes: [u8; PROFILE_BYTES],
    ) -> Result<()> {
        ctx.accounts.set_liquidity_profile(vault_idx, profile_bytes)
    }

    #[discrim = 7]
    pub fn deposit(
        ctx: &mut Context<Deposit>,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<()> {
        let (realize_event, deposit_event) = ctx.accounts.deposit(
            vault_idx,
            base_in,
            quote_in,
            max_base_in,
            max_quote_in,
            ctx.bumps.vault_depositor,
        )?;
        if let Some(re) = realize_event {
            emit_cpi!(re);
        }
        emit_cpi!(deposit_event);
        Ok(())
    }

    #[discrim = 8]
    pub fn withdraw(
        ctx: &mut Context<Withdraw>,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<()> {
        let (realize_event, withdraw_event) =
            ctx.accounts
                .withdraw(vault_idx, shares_in, min_base_out, min_quote_out)?;
        if let Some(re) = realize_event {
            emit_cpi!(re);
        }
        emit_cpi!(withdraw_event);
        Ok(())
    }

    #[discrim = 9]
    pub fn swap(
        ctx: &mut Context<Swap>,
        side: u8,
        amount_in: u64,
        limit_price_bits: u32,
        min_out: u64,
    ) -> Result<()> {
        let fill_events = ctx
            .accounts
            .swap(side, amount_in, limit_price_bits, min_out)?;
        // Per the architecture spec § Events and emission →
        // Granularity: every leg is recorded, never truncated. The
        // matching engine accumulates `FillEvent`s and we emit them
        // here one at a time via `emit_cpi!`. When the swap soft-
        // reverts (achievable output below `min_out`), no fill
        // events are produced — the loop emits nothing and the
        // instruction returns Ok so the surrounding tx can survive.
        for ev in fill_events {
            emit_cpi!(ev);
        }
        Ok(())
    }

    #[discrim = 10]
    pub fn deposit_leader(
        ctx: &mut Context<DepositLeader>,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<()> {
        let (realize_event, deposit_event) =
            ctx.accounts
                .deposit_leader(vault_idx, base_in, quote_in, max_base_in, max_quote_in)?;
        if let Some(re) = realize_event {
            emit_cpi!(re);
        }
        emit_cpi!(deposit_event);
        Ok(())
    }

    #[discrim = 11]
    pub fn withdraw_leader(
        ctx: &mut Context<WithdrawLeader>,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<()> {
        let (realize_event, withdraw_event) =
            ctx.accounts
                .withdraw_leader(vault_idx, shares_in, min_base_out, min_quote_out)?;
        if let Some(re) = realize_event {
            emit_cpi!(re);
        }
        emit_cpi!(withdraw_event);
        Ok(())
    }
}
