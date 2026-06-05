use crate::errors::DropsetError;
use crate::{
    AdminSet, Registry, DEFAULT_MAX_VAULTS_PER_MARKET, DEFAULT_MIN_LEADER_SHARE, DEFAULT_TAKER_FEE,
};
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
    // Sized for the genesis admin only; grow the slab when admin
    // management is added.
    #[account(init, payer = payer, space = Registry::space_for(1), seeds = [b"registry"], bump)]
    pub registry: Registry,
    pub system_program: Program<System>,
    /// SAFETY: the program's ProgramData account is owned by the BPF
    /// upgradeable loader, not this program, so it cannot be a typed
    /// `Account<T>`. `init()` verifies it is the canonical ProgramData
    /// PDA via `find_and_verify_program_address` and only reads its
    /// header to authenticate the upgrade authority — no data is
    /// written and no other invariant is assumed.
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

        // Init registry values. Header fields via DerefMut; the admin
        // set is the slab tail. `default_fee_config` is left zeroed —
        // no fee mint is configured at genesis.
        let registry = &mut self.registry;
        registry.bump = bump;
        registry.max_vaults_per_market = DEFAULT_MAX_VAULTS_PER_MARKET;
        registry.default_taker_fee = DEFAULT_TAKER_FEE.into();
        registry.default_min_leader_share = DEFAULT_MIN_LEADER_SHARE.into();
        // The account is pre-sized for one admin, so this seats the
        // genesis admin without growing or charging extra rent.
        registry.admin_insert(genesis_admin, self.payer.as_ref())?;
        Ok(())
    }
}
