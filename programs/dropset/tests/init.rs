mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{canonical_init_ixn, init_ixn, registry_pda};
use common::{
    assert_instruction_error, assert_program_error, associated_token_address, create_spl_mint,
    create_token2022_mint, create_token2022_token_account, decode_slab, deploy_with_authority,
    send_ixn, ATA_PROGRAM_ID, PROGRAM_ID, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID,
    TOKEN_2022_PROGRAM_ID,
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

#[test]
fn init_rejects_wrong_program_data_address() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_spl_mint(&mut svm, &authority);

    // Any pubkey other than the canonical programdata PDA — the
    // `verify_upgrade_authority` access-control hook fails the PDA
    // derivation before any data is read.
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
/// `(registry, token_program, fee_mint)` and matches the on-chain
/// shape the ATA program produces for `token_program` — SPL Token is
/// 165 bytes; Token-2022 is 165 base + 1 `AccountType` + 4 bytes for
/// the `ImmutableOwner` extension the ATA program enables by default.
fn assert_fee_vault_created(
    svm: &anchor_v2_testing::LiteSVM,
    fee_mint: Pubkey,
    token_program: Pubkey,
) {
    const SPL_TOKEN_ACCOUNT_LEN: usize = 165;
    const TOKEN_2022_ATA_LEN: usize = 170;
    let vault_addr = associated_token_address(&registry_pda(), &fee_mint, &token_program);
    let vault = svm
        .get_account(&vault_addr)
        .expect("fee vault should be created");
    assert_eq!(vault.owner, token_program, "fee vault owner mismatch");
    let expected_len = if token_program == TOKEN_2022_PROGRAM_ID {
        TOKEN_2022_ATA_LEN
    } else {
        SPL_TOKEN_ACCOUNT_LEN
    };
    assert_eq!(
        vault.data.len(),
        expected_len,
        "fee vault data length mismatch for {token_program}"
    );
    // The token account's `mint` field sits at offset 0 of its data
    // (32 bytes), then `owner` at offset 32 (32 bytes). Verify the
    // registry is the on-chain owner of the vault — i.e. only a CPI
    // signed with the registry PDA can move the funds.
    let mint_field: [u8; 32] = vault.data[..32].try_into().unwrap();
    let owner_field: [u8; 32] = vault.data[32..64].try_into().unwrap();
    assert_eq!(mint_field, fee_mint.to_bytes(), "fee vault mint mismatch");
    assert_eq!(
        owner_field,
        registry_pda().to_bytes(),
        "fee vault SPL owner is not the registry PDA"
    );
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
    assert_eq!(
        header.default_fee_config.token_program,
        SPL_TOKEN_PROGRAM_ID.to_bytes().into()
    );
    assert_eq!(header.default_fee_config.atoms.get(), TEST_FEE_ATOMS);
    // Init seeds zero markets — the counter only advances on create_market.
    assert_eq!(header.market_count.get(), 0);
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

    let account = svm.get_account(&registry_pda()).expect("registry created");
    let (header, admins) = decode_slab::<RegistryHeader, [u8; 32]>(&account.data);
    assert_eq!(header.default_fee_config.mint, fee_mint.to_bytes().into());
    assert_eq!(
        header.default_fee_config.token_program,
        TOKEN_2022_PROGRAM_ID.to_bytes().into()
    );
    assert_eq!(header.default_fee_config.atoms.get(), TEST_FEE_ATOMS);
    assert_eq!(header.market_count.get(), 0);
    assert_eq!(admins, &[genesis_admin.to_bytes()][..]);
    assert_fee_vault_created(&svm, fee_mint, TOKEN_2022_PROGRAM_ID);
}

/// A Token-2022 token account (165 bytes, not 82) passed in the mint
/// slot. `InterfaceAccount<Mint>` deserialization is the primary
/// rejection — `PodMint::unpack` reads the trailing `AccountType` byte
/// and refuses the wrong discriminator, surfacing as a runtime
/// `InvalidAccountData`.
#[test]
fn init_rejects_token2022_token_account_as_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let mint = create_token2022_mint(&mut svm, &authority);
    let token_account = create_token2022_token_account(&mut svm, &authority, &mint);

    let err = send_ixn(
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
    assert_instruction_error(&err, "InvalidAccountData");
}

/// A system-owned account passed as the mint. `InterfaceAccount<Mint>`
/// rejects it on the owner check (not Token / Token-2022), which the
/// runtime reports as `IllegalOwner`.
#[test]
fn init_rejects_non_token_mint() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);

    let bogus_mint = Pubkey::new_unique();
    svm.airdrop(&bogus_mint, SIGNER_FUNDING_LAMPORTS).unwrap();

    let err = send_ixn(
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
    assert_instruction_error(&err, "IllegalOwner");
}

/// `fee_mint` and `token_program` don't agree (SPL Token mint passed
/// alongside the Token-2022 program). The ATA program CPI's owner
/// check on the mint surfaces as `IncorrectProgramId`.
#[test]
fn init_rejects_mismatched_token_program() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let mint = create_spl_mint(&mut svm, &authority);

    let err = send_ixn(
        &mut svm,
        &authority,
        canonical_init_ixn(
            authority.pubkey(),
            Pubkey::new_unique(),
            mint,
            TEST_FEE_ATOMS,
            TOKEN_2022_PROGRAM_ID,
        ),
    )
    .expect_err("token_program / fee_mint mismatch must be rejected");
    assert_instruction_error(&err, "IncorrectProgramId");
}

/// A `fee_vault` address that isn't the canonical ATA derivation. The
/// `associated_token::*` init constraint refuses any other address —
/// reported by the runtime as `InvalidSeeds`.
#[test]
fn init_rejects_non_canonical_fee_vault() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let mint = create_spl_mint(&mut svm, &authority);

    let registry = registry_pda();
    let bogus_vault = Pubkey::new_unique();
    let ixn = Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitInstruction {
            genesis_admin: Pubkey::new_unique(),
            fee_atoms: TEST_FEE_ATOMS,
        }
        .data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(registry, false),
            AccountMeta::new_readonly(get_program_data_address(&PROGRAM_ID), false),
            AccountMeta::new_readonly(mint, false),
            AccountMeta::new(bogus_vault, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(System::id(), false),
        ],
    );
    let err =
        send_ixn(&mut svm, &authority, ixn).expect_err("non-canonical fee_vault must be rejected");
    assert_instruction_error(&err, "InvalidSeeds");
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
