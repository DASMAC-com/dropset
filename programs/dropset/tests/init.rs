mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    assert_program_error, decode_slab, deploy_with_authority, send_ixn, PROGRAM_ID,
    SIGNER_FUNDING_LAMPORTS,
};
use dropset::{
    instruction::Init as InitInstruction, DropsetError, RegistryHeader,
    DEFAULT_MAX_VAULTS_PER_MARKET, DEFAULT_MIN_LEADER_SHARE, DEFAULT_TAKER_FEE,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

fn registry_address() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn init_ixn(payer: Pubkey, genesis_admin: Pubkey, program_data: Pubkey) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitInstruction { genesis_admin }.data(),
        vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(registry_address(), false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(program_data, false),
        ],
    )
}

fn canonical_init_ixn(payer: Pubkey, genesis_admin: Pubkey) -> Instruction {
    init_ixn(payer, genesis_admin, get_program_data_address(&PROGRAM_ID))
}

#[test]
fn init_rejects_wrong_program_data_address() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);

    // Any pubkey other than the canonical programdata PDA — the address
    // verification fails before any data is read.
    let bogus = Pubkey::new_unique();
    let err = send_ixn(
        &mut svm,
        &authority,
        init_ixn(authority.pubkey(), Pubkey::new_unique(), bogus),
    )
    .expect_err("non-canonical program_data must be rejected");
    assert_program_error(&err, DropsetError::InvalidProgramDataAddress);
}

#[test]
fn init_rejects_non_upgrade_authority() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let imposter = Keypair::new();
    svm.airdrop(&imposter.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();

    let err = send_ixn(
        &mut svm,
        &imposter,
        canonical_init_ixn(imposter.pubkey(), Pubkey::new_unique()),
    )
    .expect_err("non-authority must be rejected");
    assert_program_error(&err, DropsetError::InvalidUpgradeAuthority);
}

#[test]
fn init_succeeds_for_upgrade_authority() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let genesis_admin = Pubkey::new_unique();
    let (registry_pda, registry_bump) = Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID);

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(authority.pubkey(), genesis_admin),
    )
    .expect("init should succeed");

    // Verify registry header fields + the admin tail.
    let account = svm.get_account(&registry_pda).expect("registry created");
    assert_eq!(account.owner, PROGRAM_ID, "registry not owned by program");
    let (header, admins) = decode_slab::<RegistryHeader, [u8; 32]>(&account.data);
    assert_eq!(header.bump, registry_bump);
    assert_eq!(header.max_vaults_per_market, DEFAULT_MAX_VAULTS_PER_MARKET);
    assert_eq!(header.default_taker_fee.get(), DEFAULT_TAKER_FEE);
    assert_eq!(
        header.default_min_leader_share.get(),
        DEFAULT_MIN_LEADER_SHARE
    );
    // `default_fee_config` is left zeroed at genesis (no fee mint).
    assert_eq!(header.default_fee_config.mint, <[u8; 32]>::default().into());
    assert_eq!(header.default_fee_config.atoms.get(), 0);
    // The genesis admin is the sole member of the densely-packed set.
    assert_eq!(admins, &[genesis_admin.to_bytes()][..]);
}

#[test]
fn init_rejects_second_init() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let genesis_admin = Pubkey::new_unique();

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(authority.pubkey(), genesis_admin),
    )
    .expect("first init should succeed");

    // The registry PDA now exists, so the `init` constraint must reject a
    // second initialization (the account can't be created again).
    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(authority.pubkey(), Pubkey::new_unique()),
    )
    .expect_err("registry can only be initialized once");
}
