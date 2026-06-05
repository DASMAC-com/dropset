use crate::errors::DropsetError;
use crate::{Registry, DEFAULT_MAX_SEATS_PER_MARKET};
use anchor_lang_v2::{
    address_eq,
    bytemuck::{self, Pod, Zeroable},
    find_and_verify_program_address,
    prelude::*,
};
use solana_sdk_ids::bpf_loader_upgradeable;

/// Expected `UpgradeableLoaderState::ProgramData` header.
#[repr(C, packed)]
#[derive(Copy, Clone, Pod, Zeroable)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
struct ProgramDataHeader {
    enum_tag: u32,
    slot: u64,
    upgrade_authority_present: PodBool,
    upgrade_authority: Address,
}

#[derive(Accounts)]
pub struct Init {
    #[account(mut)]
    pub payer: Signer,
    #[account(init, payer = payer, seeds = [b"registry"], bump)]
    pub registry: Account<Registry>,
    pub system_program: Program<System>,
    pub program_data: UncheckedAccount,
}

impl Init {
    #[inline(always)]
    pub fn init(&mut self, bump: u8, genesis_admin: Address, program_id: &Address) -> Result<()> {
        let program_data_account = self.program_data.account();

        // Verify the program data account.
        find_and_verify_program_address(
            &[program_id.as_ref()],
            &bpf_loader_upgradeable::ID,
            self.program_data.address(),
        )
        .map_err(|_| DropsetError::InvalidProgramDataAddress)?;

        // Get upgrade authority.
        let upgrade_authority = program_data_account
            .try_borrow()?
            .get(..core::mem::size_of::<ProgramDataHeader>())
            .map(bytemuck::from_bytes::<ProgramDataHeader>)
            .ok_or(DropsetError::InvalidProgramData)?
            .upgrade_authority;

        // Verify upgrade authority.
        if !address_eq(&upgrade_authority, self.payer.address()) {
            return Err(DropsetError::InvalidUpgradeAuthority.into());
        }

        // Init registry values.
        let registry = &mut self.registry;
        registry.max_seats_per_market = DEFAULT_MAX_SEATS_PER_MARKET;
        registry.bump = bump;
        registry.admins.push(genesis_admin);
        Ok(())
    }
}
