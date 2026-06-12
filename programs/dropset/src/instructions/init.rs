use crate::errors::DropsetError;
use crate::{
    AdminSet, FeeConfig, Registry, DEFAULT_MAX_VAULTS_PER_MARKET, DEFAULT_MIN_LEADER_SHARE,
    DEFAULT_TAKER_FEE,
};
use anchor_lang_v2::{
    address_eq,
    bytemuck::{self, Pod, Zeroable},
    find_and_verify_program_address,
    prelude::*,
};
use anchor_spl_v2::{
    associated_token::AssociatedToken,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

// cspell:ignore BPFLoaderUpgradeab
/// BPF upgradeable loader program ID — the owner of every program's
/// `ProgramData` account. Typed as [`Address`] so the PDA
/// verification reads cleanly; the `solana-sdk-ids` re-export is a
/// `Pubkey` that would otherwise need a conversion at every call site.
const BPF_LOADER_UPGRADEABLE_ID: Address =
    anchor_lang_v2::address!("BPFLoaderUpgradeab1e11111111111111111111111");

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
    // Sized for the genesis admin only; `admin_insert` grows the slab
    // dynamically when more admins are added.
    #[account(init, payer = payer, space = Registry::space_for(1), seeds = [b"registry"], bump)]
    pub registry: Registry,
    /// SAFETY: the program's ProgramData account is owned by the BPF
    /// upgradeable loader, not this program, so it cannot be a typed
    /// `Account<T>`. `verify_upgrade_authority` re-derives the
    /// canonical `ProgramData` PDA and only reads its header to
    /// authenticate the upgrade authority — no data is written and
    /// no other invariant is assumed. A declarative
    /// `seeds = [crate::ID.as_ref()], seeds::program = …` would emit
    /// an opaque `{"kind":"expr"}` seed that anchor v2's IDL spec
    /// can't deserialize today; the manual check sidesteps that.
    pub program_data: UncheckedAccount,
    /// The mint to charge fees in. `InterfaceAccount<Mint>` validates
    /// SPL Token / Token-2022 ownership and that the data unpacks as a
    /// `Mint` — so no separate length / discriminator check is needed.
    pub fee_mint: InterfaceAccount<Mint>,
    /// Registry-owned fee vault for the per-`CreateVault` charge. Created
    /// here by CPI to the ATA program; its address is the canonical
    /// ATA over `(registry, token_program, fee_mint)`, and the ATA
    /// program rejects any `(mint, token_program)` pair whose owners
    /// disagree — a second backstop after `InterfaceAccount<Mint>`.
    #[account(
        init,
        payer = payer,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = token_program,
    )]
    pub fee_vault: InterfaceAccount<TokenAccount>,
    /// Token program owning `fee_mint` — SPL Token or Token-2022.
    /// `Interface<TokenInterface>` rejects any other address up front
    /// (`IncorrectProgramId`); the ATA seeds bake this in too, so a
    /// caller-supplied mismatch against `fee_mint`'s owner yields a
    /// non-canonical ATA and a separate `InvalidSeeds` rejection.
    pub token_program: Interface<'static, TokenInterface>,
    pub associated_token_program: Program<AssociatedToken>,
    pub system_program: Program<System>,
}

impl Init {
    /// Pre-handler check used via `#[access_control]` on the
    /// `init` dispatcher: pin `program_data` to this program's
    /// canonical `ProgramData` PDA, decode its header, and reject
    /// the instruction unless `payer` is the program's upgrade
    /// authority. Lives here rather than inline in the handler so
    /// the body below stays focused on stamping registry state.
    pub fn verify_upgrade_authority(&self, program_id: &Address) -> Result<()> {
        find_and_verify_program_address(
            &[program_id.as_ref()],
            &BPF_LOADER_UPGRADEABLE_ID,
            self.program_data.address(),
        )
        .map_err(|_| DropsetError::InvalidProgramDataAddress)?;
        let upgrade_authority = self
            .program_data
            .account()
            .try_borrow()?
            .get(..core::mem::size_of::<ProgramDataHeader>())
            .map(bytemuck::from_bytes::<ProgramDataHeader>)
            .ok_or(DropsetError::InvalidProgramData)?
            .upgrade_authority;
        if !address_eq(&upgrade_authority, self.payer.address()) {
            return Err(DropsetError::InvalidUpgradeAuthority.into());
        }
        Ok(())
    }

    #[inline(always)]
    pub fn init(&mut self, bump: u8, genesis_admin: Address, fee_atoms: u64) -> Result<()> {
        // The program-data PDA is verified by the `seeds` constraint
        // and the upgrade-authority check fires via `#[access_control]`
        // before this body runs — see `lib.rs::init`.

        // Init registry values. Header fields via DerefMut; the admin
        // set is the slab tail.
        let registry = &mut self.registry;
        registry.bump = bump;
        registry.max_vaults_per_market = DEFAULT_MAX_VAULTS_PER_MARKET;
        registry.default_taker_fee = DEFAULT_TAKER_FEE.into();
        registry.default_min_leader_share = DEFAULT_MIN_LEADER_SHARE.into();
        // No markets exist at init; `create_market` will increment, and
        // `close_registry` (under the `admin-teardown` feature) checks
        // this is zero before the registry can be closed.
        registry.market_count = 0u32.into();
        registry.default_fee_config = FeeConfig {
            mint: *self.fee_mint.address(),
            token_program: *self.token_program.address(),
            atoms: fee_atoms.into(),
        };
        // The account is pre-sized for one admin, so this seats the
        // genesis admin without growing or charging extra rent.
        registry.admin_insert(genesis_admin, self.payer.as_ref())?;
        Ok(())
    }
}
