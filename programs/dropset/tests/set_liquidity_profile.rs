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

use anchor_lang_v2::bytemuck;
use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
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
use dropset::{LiquidityProfile, Price, FLUSH_BIT};
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
    // `#[event_cpi]` on the `RegisterVault` Accounts struct appends
    // two accounts: the event-authority PDA (seeds = `__event_authority`)
    // and the program account itself, used by `emit_cpi!`'s self-CPI.
    let event_authority = Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0;
    let register_vault_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &RegisterVaultInstruction {
            perf_fee_rate: 0,
            quote_authority: authority.pubkey(),
            allow_outside_depositors: false,
            leader_override: Pubkey::default(),
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
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
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

// ── Fixture-based expansion (WI2) ────────────────────────────────────

/// Open an admin vault (sector 0) with a reference price already set,
/// so `set_liquidity_profile` clears its `ReferencePriceNotSet` gate.
fn fixture_with_priced_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f
}

/// Profile with two levels on one side summing to `> BPS` (10_000).
fn oversized_profile(bid_side: bool) -> [u8; PROFILE_BYTES] {
    let mut p: LiquidityProfile = bytemuck::Zeroable::zeroed();
    let levels = if bid_side { &mut p.bids } else { &mut p.asks };
    levels[0].size_bps = 6_000u16.into();
    levels[1].size_bps = 5_000u16.into(); // 11_000 > 10_000
    let mut bytes = [0u8; PROFILE_BYTES];
    bytes.copy_from_slice(bytemuck::bytes_of(&p));
    bytes
}

#[test]
fn happy_path_writes_profile_arms_flush_keeps_price() {
    let mut f = fixture_with_priced_vault();
    let before = f.vault(0).reference_price;
    let signer = f.authority.insecure_clone();

    f.set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("leader writes profile");

    let v = f.vault(0);
    assert_eq!(v.profile.asks[0].size_bps.get(), 10_000, "profile written");
    assert_eq!(v.profile.asks[0].price_offset.get(), 5_000);
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT re-armed"
    );
    assert_eq!(
        v.reference_price.price.as_u32(),
        before.price.as_u32(),
        "reference price unchanged"
    );
    assert_eq!(
        v.reference_price.quote_slot.get(),
        before.quote_slot.get(),
        "quote_slot unchanged"
    );
}

#[test]
fn rejects_bid_size_overflow() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    let err = f
        .set_liquidity_profile(&signer, 0, oversized_profile(true))
        .expect_err("Σ bid size_bps > 10_000 must reject");
    common::assert_program_error(&err, DropsetError::LiquidityProfileSizeOverflow);
}

#[test]
fn rejects_ask_size_overflow() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    let err = f
        .set_liquidity_profile(&signer, 0, oversized_profile(false))
        .expect_err("Σ ask size_bps > 10_000 must reject");
    common::assert_program_error(&err, DropsetError::LiquidityProfileSizeOverflow);
}

#[test]
fn rejects_unauthorized_signer() {
    let mut f = fixture_with_priced_vault();
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_liquidity_profile(&stranger, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect_err("non quote-authority must reject");
    common::assert_program_error(&err, DropsetError::Unauthorized);
}

#[test]
fn rejects_frozen_vault() {
    let mut f = fixture_with_priced_vault();
    f.poke_frozen(0, true);
    let signer = f.authority.insecure_clone();
    let err = f
        .set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect_err("frozen vault must reject a profile update");
    common::assert_program_error(&err, DropsetError::VaultFrozen);
}
