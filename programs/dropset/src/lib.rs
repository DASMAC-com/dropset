// cspell:word discrim
use anchor_lang_v2::prelude::*;

mod errors;
mod events;
mod instructions;
mod state;

pub use errors::*;
pub use events::*;
use instructions::*;
pub use state::*;

// The `Price` codec lives in the solana-free `dropset-price-core` crate
// (interface.md § SDK) so the on-chain engine, the Rust SDK, and the WASM
// client all share one implementation. Re-exported at the crate root so
// existing `crate::Price` paths keep resolving.
pub use dropset_price_core::Price;

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
    pub fn create_market(ctx: &mut Context<CreateMarket>) -> Result<()> {
        ctx.accounts.create_market(ctx.bumps.market)
    }

    #[discrim = 4]
    pub fn create_vault(
        ctx: &mut Context<CreateVault>,
        perf_fee_rate: u32,
        quote_authority: Address,
        allow_outside_depositors: bool,
        leader_override: Address,
    ) -> Result<()> {
        let event = ctx.accounts.create_vault(
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

    #[discrim = 12]
    pub fn set_allow_outside_depositors(
        ctx: &mut Context<SetAllowOutsideDepositors>,
        vault_idx: u32,
        flag: bool,
    ) -> Result<()> {
        ctx.accounts.set_allow_outside_depositors(vault_idx, flag)
    }

    #[discrim = 13]
    pub fn set_outside_deposits_approved(
        ctx: &mut Context<SetOutsideDepositsApproved>,
        vault_idx: u32,
        flag: bool,
    ) -> Result<()> {
        ctx.accounts.set_outside_deposits_approved(vault_idx, flag)
    }

    #[discrim = 14]
    pub fn close_vault(ctx: &mut Context<CloseVault>, vault_idx: u32) -> Result<()> {
        let event = ctx.accounts.close_vault(vault_idx)?;
        emit_cpi!(event);
        Ok(())
    }

    #[discrim = 15]
    pub fn freeze_vault(ctx: &mut Context<FreezeVault>, vault_idx: u32) -> Result<()> {
        let event = ctx.accounts.freeze_vault(vault_idx)?;
        emit_cpi!(event);
        Ok(())
    }

    // ── Teardown surface ─────────────────────────────────────────────
    // Always wired into the program, but each handler short-circuits to
    // `DropsetError::TeardownDisabled` unless the `admin-teardown`
    // feature is on. The feature is on for testnet / early-mainnet
    // builds and off for the final immutable deploy, which keeps the
    // instructions present-but-inert there. anchor v2's `#[program]`
    // macro doesn't propagate `#[cfg]` onto its generated dispatch glue,
    // so a per-instruction compile-out isn't available — this runtime
    // guard is the supported alternative. See the architecture spec,
    // § Account lifecycle and rent reclamation.

    #[discrim = 16]
    pub fn force_withdraw_depositor(
        ctx: &mut Context<ForceWithdrawDepositor>,
        vault_idx: u32,
    ) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = (&ctx, vault_idx);
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            let (realize_event, withdraw_event) =
                ctx.accounts.force_withdraw_depositor(vault_idx)?;
            if let Some(re) = realize_event {
                emit_cpi!(re);
            }
            emit_cpi!(withdraw_event);
            Ok(())
        }
    }

    #[discrim = 17]
    pub fn force_withdraw_leader(
        ctx: &mut Context<ForceWithdrawLeader>,
        vault_idx: u32,
    ) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = (&ctx, vault_idx);
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            let (realize_event, withdraw_event) = ctx.accounts.force_withdraw_leader(vault_idx)?;
            if let Some(re) = realize_event {
                emit_cpi!(re);
            }
            emit_cpi!(withdraw_event);
            Ok(())
        }
    }

    #[discrim = 18]
    pub fn close_market_treasury(ctx: &mut Context<CloseMarketTreasury>) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = &ctx;
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            ctx.accounts.close_market_treasury()
        }
    }

    #[discrim = 19]
    pub fn close_market(ctx: &mut Context<CloseMarket>) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = &ctx;
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            ctx.accounts.close_market()
        }
    }

    #[discrim = 20]
    pub fn close_registry_fee_vault(ctx: &mut Context<CloseRegistryFeeVault>) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = &ctx;
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            ctx.accounts.close_registry_fee_vault()
        }
    }

    #[discrim = 21]
    pub fn close_registry(ctx: &mut Context<CloseRegistry>) -> Result<()> {
        #[cfg(not(feature = "admin-teardown"))]
        {
            let _ = &ctx;
            Err(DropsetError::TeardownDisabled.into())
        }
        #[cfg(feature = "admin-teardown")]
        {
            ctx.accounts.close_registry()
        }
    }

    // ── Post-create admin retuning levers ────────────────────────────
    // Always-on admin mutators (not teardown-gated) that retune values
    // stamped once at create time — a vault's `min_leader_share` floor
    // and a market's create-vault `fee_config`. Appended after the
    // teardown surface so discriminants 0–21 keep their numbers: clients
    // key instructions on the discriminant, so inserting mid-list would
    // break every existing client. See the architecture spec,
    // § SetMinLeaderShare and § SetMarketFeeConfig.

    #[discrim = 22]
    pub fn set_min_leader_share(
        ctx: &mut Context<SetMinLeaderShare>,
        vault_idx: u32,
        min_leader_share: u32,
    ) -> Result<()> {
        let event = ctx
            .accounts
            .set_min_leader_share(vault_idx, min_leader_share)?;
        emit_cpi!(event);
        Ok(())
    }

    #[discrim = 23]
    pub fn set_market_fee_config(
        ctx: &mut Context<SetMarketFeeConfig>,
        atoms: u64,
    ) -> Result<()> {
        let event = ctx.accounts.set_market_fee_config(atoms)?;
        emit_cpi!(event);
        Ok(())
    }
}
