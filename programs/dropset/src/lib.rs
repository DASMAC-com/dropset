// cspell:word discrim
use anchor_lang_v2::prelude::*;

mod errors;
mod instructions;
mod state;

pub use errors::*;
use instructions::*;
pub use state::*;

declare_id!("TESTnXwv2eHoftsSd5NEdpH4zEu7XRC8jviuoNPdB2Q");

#[program]
pub mod dropset {
    use super::*;

    #[discrim = 0]
    pub fn init(ctx: &mut Context<Init>, genesis_admin: Address) -> Result<()> {
        ctx.accounts
            .init(ctx.bumps.registry, genesis_admin, ctx.program_id)
    }
}
