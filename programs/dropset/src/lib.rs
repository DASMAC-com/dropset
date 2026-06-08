// cspell:word discrim
use anchor_lang_v2::prelude::*;

mod errors;
mod instructions;
mod price;
mod state;

pub use errors::*;
use instructions::*;
pub use price::*;
pub use state::*;

declare_id!("TESTnXwv2eHoftsSd5NEdpH4zEu7XRC8jviuoNPdB2Q");

#[program]
pub mod dropset {
    use super::*;

    #[discrim = 0]
    pub fn init(ctx: &mut Context<Init>, genesis_admin: Address, fee_atoms: u64) -> Result<()> {
        ctx.accounts
            .init(ctx.bumps.registry, genesis_admin, fee_atoms, ctx.program_id)
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
}
