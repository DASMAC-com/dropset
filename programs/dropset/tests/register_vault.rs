//! `register_vault` integration tests — admin leader-override path,
//! non-admin rejection, and the cap-exceeded gate. Each test does the
//! init + register_market bootstrap up front.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, deploy_with_authority, mint_to, send_ixn, ATA_PROGRAM_ID, PROGRAM_ID,
    REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID,
};
use dropset::{
    instruction::{
        Init as InitInstruction, RegisterMarket as RegisterMarketInstruction,
        RegisterVault as RegisterVaultInstruction,
    },
    DropsetError, MarketHeader, Vault,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}
fn market_pda(b: &Pubkey, q: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b.as_ref(), q.as_ref()], &PROGRAM_ID).0
}
fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

fn read_first_vault(svm: &anchor_v2_testing::LiteSVM, market: &Pubkey) -> Vault {
    let acct = svm.get_account(market).expect("market");
    const DISC: usize = 8;
    let header_end = DISC + core::mem::size_of::<MarketHeader>();
    let after_len = header_end + 4;
    let v_align = core::mem::align_of::<Vault>();
    let items_start = (after_len + v_align - 1) & !(v_align - 1);
    let v_size = core::mem::size_of::<Vault>();
    anchor_lang_v2::bytemuck::pod_read_unaligned::<Vault>(
        &acct.data[items_start..items_start + v_size],
    )
}

#[allow(clippy::type_complexity)]
fn bootstrap_market() -> (
    anchor_v2_testing::LiteSVM,
    Keypair,
    Pubkey,
    Pubkey,
    Pubkey,
    Pubkey,
) {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_mock_usdc_mint(&mut svm, &authority);
    let registry = registry_pda();
    let fee_vault = associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    let init_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitInstruction {
            genesis_admin: authority.pubkey(),
            fee_atoms: REGISTER_MARKET_FEE_ATOMS,
        }
        .data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(registry, false),
            AccountMeta::new_readonly(get_program_data_address(&PROGRAM_ID), false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new(fee_vault, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(System::id(), false),
        ],
    );
    send_ixn(&mut svm, &authority, init_ix).expect("init");
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    let market = market_pda(&base_mint, &quote_mint);
    let registry_fee_treasury =
        associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    let base_tr = associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID);
    let quote_tr = associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID);
    let dummy = Keypair::new();
    svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS).unwrap();
    let ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &RegisterMarketInstruction {}.data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(registry, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new_readonly(quote_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(market, false),
            AccountMeta::new(base_tr, false),
            AccountMeta::new(quote_tr, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(dummy.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, ix).expect("register_market");
    (svm, authority, market, fee_mint, base_mint, quote_mint)
}

#[allow(clippy::too_many_arguments)]
fn register_vault_for(
    market: &Pubkey,
    payer: &Pubkey,
    fee_mint: &Pubkey,
    payer_fee_source: &Pubkey,
    perf_fee_rate: u32,
    quote_authority: &Pubkey,
    leader_override: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    let registry_fee_treasury =
        associated_token_address(&registry, fee_mint, &SPL_TOKEN_PROGRAM_ID);
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &RegisterVaultInstruction {
            perf_fee_rate,
            quote_authority: *quote_authority,
            allow_outside_depositors: false,
            leader_override: *leader_override,
        }
        .data(),
        vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(registry, false),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(*fee_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(*payer_fee_source, false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(event_authority(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    )
}

#[test]
fn admin_can_open_vault_for_another_leader() {
    let (mut svm, authority, market, fee_mint, _base_mint, _quote_mint) = bootstrap_market();
    // Admin opens a vault with a foreign pubkey as leader.
    let foreign = Keypair::new();
    let dummy = Keypair::new();
    svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS).unwrap();
    let ix = register_vault_for(
        &market,
        &authority.pubkey(),
        &fee_mint,
        &dummy.pubkey(),
        100_000, // 10% perf fee
        &foreign.pubkey(),
        &foreign.pubkey(), // admin override
    );
    send_ixn(&mut svm, &authority, ix).expect("admin leader-override should succeed");

    let vault = read_first_vault(&svm, &market);
    assert_eq!(vault.leader, foreign.pubkey().to_bytes().into());
    assert_eq!(vault.quote_authority, foreign.pubkey().to_bytes().into());
    assert_eq!(vault.perf_fee_rate.get(), 100_000);
}

#[test]
fn non_admin_with_foreign_leader_override_rejects() {
    let (mut svm, authority, market, fee_mint, _base_mint, _quote_mint) = bootstrap_market();
    // Fresh non-admin payer. Doesn't matter that they're underfunded
    // for the fee — the override-mismatch check fires before the
    // fee transfer.
    let outsider = Keypair::new();
    svm.airdrop(&outsider.pubkey(), 10 * SIGNER_FUNDING_LAMPORTS)
        .unwrap();
    // Pre-fund their fee ATA so the rejection isn't masked by an
    // insufficient-funds fee transfer.
    let outsider_fee_ata = create_associated_token_account(
        &mut svm,
        &authority,
        &outsider.pubkey(),
        &fee_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(
        &mut svm,
        &authority,
        &fee_mint,
        &outsider_fee_ata,
        REGISTER_MARKET_FEE_ATOMS,
    );
    let foreign = Keypair::new();
    let ix = register_vault_for(
        &market,
        &outsider.pubkey(),
        &fee_mint,
        &outsider_fee_ata,
        0,
        &outsider.pubkey(),
        &foreign.pubkey(), // override to someone NOT the payer
    );
    let err = send_ixn(&mut svm, &outsider, ix)
        .expect_err("non-admin override to foreign pubkey must reject");
    common::assert_program_error(&err, DropsetError::LeaderOverrideNotAllowed);
}

#[test]
fn invalid_perf_fee_rate_rejects() {
    let (mut svm, authority, market, fee_mint, _base_mint, _quote_mint) = bootstrap_market();
    let dummy = Keypair::new();
    svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS).unwrap();
    // perf_fee_rate > 1_000_000 ppm = > 100% — rejected.
    let ix = register_vault_for(
        &market,
        &authority.pubkey(),
        &fee_mint,
        &dummy.pubkey(),
        1_000_001,
        &authority.pubkey(),
        &Pubkey::default(),
    );
    let err = send_ixn(&mut svm, &authority, ix)
        .expect_err("perf_fee_rate > 1_000_000 must reject");
    common::assert_program_error(&err, DropsetError::InvalidPerfFeeRate);
}
