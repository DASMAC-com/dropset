mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    assert_program_error, associated_token_address, create_spl_mint, create_token2022_mint,
    create_token2022_token_account, decode_slab, deploy_with_authority, send_ixn, ATA_PROGRAM_ID,
    PROGRAM_ID, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
};
use dropset::{
    instruction::Init as InitInstruction, DropsetError, RegistryHeader,
    DEFAULT_MAX_VAULTS_PER_MARKET, DEFAULT_MIN_LEADER_SHARE, DEFAULT_TAKER_FEE,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

/// Fee atoms used across init tests (1k with 6 decimals).
const TEST_FEE_ATOMS: u64 = 1_000_000_000;

fn registry_address() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn init_ixn(
    payer: Pubkey,
    genesis_admin: Pubkey,
    fee_mint: Pubkey,
    fee_atoms: u64,
    program_data: Pubkey,
    token_program: Pubkey,
) -> Instruction {
    let registry = registry_address();
    let fee_vault = associated_token_address(&registry, &fee_mint, &token_program);
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitInstruction {
            genesis_admin,
            fee_atoms,
        }
        .data(),
        vec![
            AccountMeta::new(payer, true),
            AccountMeta::new(registry, false),
            AccountMeta::new_readonly(program_data, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new(fee_vault, false),
            AccountMeta::new_readonly(token_program, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(System::id(), false),
        ],
    )
}

fn canonical_init_ixn(
    payer: Pubkey,
    genesis_admin: Pubkey,
    fee_mint: Pubkey,
    fee_atoms: u64,
    token_program: Pubkey,
) -> Instruction {
    init_ixn(
        payer,
        genesis_admin,
        fee_mint,
        fee_atoms,
        get_program_data_address(&PROGRAM_ID),
        token_program,
    )
}

#[test]
fn init_rejects_wrong_program_data_address() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_spl_mint(&mut svm, &authority);

    // Any pubkey other than the canonical programdata PDA — the address
    // verification fails before any data is read.
    let bogus = Pubkey::new_unique();
    let err = send_ixn(
        &mut svm,
        &authority,
        init_ixn(
            authority.pubkey(),
            Pubkey::new_unique(),
            fee_mint,
            TEST_FEE_ATOMS,
            bogus,
            SPL_TOKEN_PROGRAM_ID,
        ),
    )
    .expect_err("non-canonical program_data must be rejected");
    assert_program_error(&err, DropsetError::InvalidProgramDataAddress);
}

#[test]
fn init_rejects_non_upgrade_authority() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_spl_mint(&mut svm, &authority);
    let imposter = Keypair::new();
    svm.airdrop(&imposter.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();

    let err = send_ixn(
        &mut svm,
        &imposter,
        canonical_init_ixn(
            imposter.pubkey(),
            Pubkey::new_unique(),
            fee_mint,
            TEST_FEE_ATOMS,
            SPL_TOKEN_PROGRAM_ID,
        ),
    )
    .expect_err("non-authority must be rejected");
    assert_program_error(&err, DropsetError::InvalidUpgradeAuthority);
}

/// Assert the registry's fee vault was created as the ATA over
/// `(registry, token_program, fee_mint)` and is owned by `token_program`.
fn assert_fee_vault_created(
    svm: &anchor_v2_testing::LiteSVM,
    fee_mint: Pubkey,
    token_program: Pubkey,
) {
    let vault_addr = associated_token_address(&registry_address(), &fee_mint, &token_program);
    let vault = svm
        .get_account(&vault_addr)
        .expect("fee vault should be created");
    assert_eq!(vault.owner, token_program, "fee vault owner mismatch");
    assert!(!vault.data.is_empty(), "fee vault data is empty");
}

#[test]
fn init_succeeds_with_spl_token_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let genesis_admin = Pubkey::new_unique();
    let fee_mint = create_spl_mint(&mut svm, &authority);
    let (registry_pda, registry_bump) = Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID);

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            genesis_admin,
            fee_mint,
            TEST_FEE_ATOMS,
            SPL_TOKEN_PROGRAM_ID,
        ),
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
    // Fee config matches the mint account and atoms passed to init.
    assert_eq!(header.default_fee_config.mint, fee_mint.to_bytes().into());
    assert_eq!(header.default_fee_config.atoms.get(), TEST_FEE_ATOMS);
    // The genesis admin is the sole member of the densely-packed set.
    assert_eq!(admins, &[genesis_admin.to_bytes()][..]);
    assert_fee_vault_created(&svm, fee_mint, SPL_TOKEN_PROGRAM_ID);
}

#[test]
fn init_succeeds_with_token2022_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let genesis_admin = Pubkey::new_unique();
    let fee_mint = create_token2022_mint(&mut svm, &authority);

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            genesis_admin,
            fee_mint,
            TEST_FEE_ATOMS,
            TOKEN_2022_PROGRAM_ID,
        ),
    )
    .expect("init with Token-2022 mint should succeed");

    let account = svm
        .get_account(&registry_address())
        .expect("registry created");
    let (header, admins) = decode_slab::<RegistryHeader, [u8; 32]>(&account.data);
    assert_eq!(header.default_fee_config.mint, fee_mint.to_bytes().into());
    assert_eq!(header.default_fee_config.atoms.get(), TEST_FEE_ATOMS);
    assert_eq!(admins, &[genesis_admin.to_bytes()][..]);
    assert_fee_vault_created(&svm, fee_mint, TOKEN_2022_PROGRAM_ID);
}

/// A Token-2022 token account (165 bytes, not 82) passed in the mint
/// slot. Validation now happens via `InterfaceAccount<Mint>` /
/// the ATA-init CPI rather than a manual length check, so we only
/// assert the call is rejected — no specific custom error code.
#[test]
fn init_rejects_token2022_token_account_as_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let mint = create_token2022_mint(&mut svm, &authority);
    let token_account = create_token2022_token_account(&mut svm, &authority, &mint);

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            Pubkey::new_unique(),
            token_account,
            TEST_FEE_ATOMS,
            TOKEN_2022_PROGRAM_ID,
        ),
    )
    .expect_err("token account must not be accepted as a fee mint");
}

/// A system-owned account passed as the mint — neither owned by SPL
/// Token nor Token-2022. As above, we only assert init refuses it.
#[test]
fn init_rejects_non_token_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);

    let bogus_mint = Pubkey::new_unique();
    svm.airdrop(&bogus_mint, SIGNER_FUNDING_LAMPORTS).unwrap();

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            Pubkey::new_unique(),
            bogus_mint,
            TEST_FEE_ATOMS,
            SPL_TOKEN_PROGRAM_ID,
        ),
    )
    .expect_err("non-token mint must be rejected");
}

#[test]
fn init_rejects_second_init() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let genesis_admin = Pubkey::new_unique();
    let fee_mint = create_spl_mint(&mut svm, &authority);

    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            genesis_admin,
            fee_mint,
            TEST_FEE_ATOMS,
            SPL_TOKEN_PROGRAM_ID,
        ),
    )
    .expect("first init should succeed");

    // The registry PDA now exists, so the `init` constraint must reject a
    // second initialization (the account can't be created again).
    send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            Pubkey::new_unique(),
            fee_mint,
            TEST_FEE_ATOMS,
            SPL_TOKEN_PROGRAM_ID,
        ),
    )
    .expect_err("registry can only be initialized once");
}
