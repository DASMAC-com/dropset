//! Integration tests for `set_liquidity_profile`. The MVP rule under
//! test: the instruction must reject when the vault's reference price
//! has not yet been set (i.e. on a freshly-opened vault). This is the
//! user-requested gate that lets the spec stay clean — the profile is
//! pure ppm offsets, so applying it before a real anchor price would
//! flush to garbage absolute prices.
//!
//! The happy path is asserted in the end-to-end flow in `swap.rs`
//! (lands with that test in the same PR series); this file isolates
//! the rejection path so a regression of the gate fails loudly.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    associated_token_address, create_mock_usdc_mint, create_spl_mint, deploy_with_authority,
    send_ixn, ATA_PROGRAM_ID, PROGRAM_ID, REGISTER_MARKET_FEE_ATOMS, SPL_TOKEN_PROGRAM_ID,
};
use dropset::{
    instruction::{
        Init as InitInstruction, RegisterMarket as RegisterMarketInstruction,
        RegisterVault as RegisterVaultInstruction,
        SetLiquidityProfile as SetLiquidityProfileInstruction,
    },
    DropsetError, N_LEVELS,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn market_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[base_mint.as_ref(), quote_mint.as_ref()], &PROGRAM_ID)
}

#[test]
fn rejects_when_reference_price_not_set() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_mock_usdc_mint(&mut svm, &authority);

    // Init the registry.
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
    send_ixn(&mut svm, &authority, init_ix).expect("registry init");

    // Register a market with two fresh SPL mints, both under the
    // standard SPL Token program.
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    let (market, _market_bump) = market_pda(&base_mint, &quote_mint);
    let base_treasury = associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID);
    let quote_treasury = associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID);
    let registry_fee_treasury =
        associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    // Anchor v2 rejects duplicate-mut accounts, so the
    // `payer_fee_source` field can't reuse `authority` even though the
    // admin branch never reads it. Use a fresh throwaway pubkey instead.
    let dummy_source = Keypair::new();
    svm.airdrop(&dummy_source.pubkey(), 1_000_000).unwrap();
    // Admin opens the market — fee path is waived.
    let register_market_ix = Instruction::new_with_bytes(
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
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(dummy_source.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, register_market_ix).expect("register_market");

    // Admin opens the vault for simplicity — skips fee path.
    let register_vault_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &RegisterVaultInstruction {
            perf_fee_rate: 0,
            quote_authority: authority.pubkey().into(),
            allow_outside_depositors: false,
        }
        .data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(registry, false),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(dummy_source.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
        ],
    );
    send_ixn(&mut svm, &authority, register_vault_ix).expect("register_vault");

    // Now: attempt set_liquidity_profile BEFORE set_reference_price.
    // The vault was just allocated; its reference_price.price is the
    // zero sentinel. The gate should reject the call.
    let mut profile_bytes = [0u8; 2 * N_LEVELS * 10];
    // A nominal level entry — value doesn't matter, the gate fires
    // before the size_bps invariant check.
    profile_bytes[0] = 0; // bid level 0 price_offset = 0

    let set_profile_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &SetLiquidityProfileInstruction {
            vault_idx: 0,
            profile_bytes,
        }
        .data(),
        vec![
            AccountMeta::new_readonly(authority.pubkey(), true),
            AccountMeta::new(market, false),
        ],
    );
    let err = send_ixn(&mut svm, &authority, set_profile_ix)
        .expect_err("set_liquidity_profile must reject before set_reference_price");
    common::assert_program_error(&err, DropsetError::ReferencePriceNotSet);
}
