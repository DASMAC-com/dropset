//! Swap integration tests — exercise the multi-vault heap matcher +
//! `min_out` soft-revert behavior end-to-end against the deployed
//! `.so`. Each test builds the leader pipeline (`init` →
//! `register_market` → `register_vault` → `set_reference_price` →
//! `set_liquidity_profile` → seed via `deposit_leader`), then runs a
//! `swap` against the seeded vault and asserts the outcome.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, decode_slab, deploy_with_authority, mint_to, send_ixn,
    ATA_PROGRAM_ID, PROGRAM_ID, REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS,
    SPL_TOKEN_PROGRAM_ID,
};
use dropset::{
    instruction::{
        DepositLeader as DepositLeaderInstruction, Init as InitInstruction,
        RegisterMarket as RegisterMarketInstruction, RegisterVault as RegisterVaultInstruction,
        SetLiquidityProfile as SetLiquidityProfileInstruction,
        SetReferencePrice as SetReferencePriceInstruction, Swap as SwapInstruction,
    },
    LiquidityProfile, MarketHeader, Price, Vault, N_LEVELS,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

const SYSVAR_CLOCK_ID: Pubkey =
    Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}
fn market_pda(b: &Pubkey, q: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[b.as_ref(), q.as_ref()], &PROGRAM_ID).0
}
fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

fn token_balance(svm: &anchor_v2_testing::LiteSVM, ata: &Pubkey) -> u64 {
    let acct = svm.get_account(ata).expect("token account exists");
    u64::from_le_bytes(acct.data[64..72].try_into().unwrap())
}

fn read_first_vault(svm: &anchor_v2_testing::LiteSVM, market: &Pubkey) -> (MarketHeader, Vault) {
    let acct = svm.get_account(market).expect("market");
    const DISC: usize = 8;
    let header_end = DISC + core::mem::size_of::<MarketHeader>();
    let header = anchor_lang_v2::bytemuck::pod_read_unaligned::<MarketHeader>(
        &acct.data[DISC..header_end],
    );
    let after_len = header_end + 4;
    let v_align = core::mem::align_of::<Vault>();
    let items_start = (after_len + v_align - 1) & !(v_align - 1);
    let v_size = core::mem::size_of::<Vault>();
    let vault = anchor_lang_v2::bytemuck::pod_read_unaligned::<Vault>(
        &acct.data[items_start..items_start + v_size],
    );
    (header, vault)
}

/// Stand up a seeded vault with a non-trivial ladder ready for `swap`
/// to match against. Returns `(svm, authority, market, base_mint,
/// quote_mint, base_treasury, quote_treasury)`.
#[allow(clippy::type_complexity)]
fn setup_seeded_vault() -> (
    anchor_v2_testing::LiteSVM,
    Keypair,
    Pubkey,
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

    // init.
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

    // register_market.
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    let market = market_pda(&base_mint, &quote_mint);
    let base_treasury = associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID);
    let quote_treasury = associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID);
    let registry_fee_treasury =
        associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
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
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(dummy.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, ix).expect("register_market");

    // register_vault (admin path, no fee).
    let ix = Instruction::new_with_bytes(
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
            AccountMeta::new(dummy.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(event_authority(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, ix).expect("register_vault");

    // set_reference_price (EUR/USD = 1.0850).
    let ref_price = Price::encode(10_850_000, 0).unwrap();
    let ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &SetReferencePriceInstruction {
            vault_idx: 0,
            price_bits: ref_price.as_u32(),
            quote_slot: 0,
        }
        .data(),
        vec![
            AccountMeta::new_readonly(authority.pubkey(), true),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, ix).expect("set_reference_price");

    // set_liquidity_profile — one bid + one ask, ±0.05% offset, full
    // inventory (10_000 bps), long expiry.
    let mut profile: LiquidityProfile = anchor_lang_v2::bytemuck::Zeroable::zeroed();
    profile.bids[0].price_offset = 5_000u32.into();
    profile.bids[0].size_bps = 10_000u16.into();
    profile.bids[0].expiry_offset = u32::MAX.into();
    profile.asks[0].price_offset = 5_000u32.into();
    profile.asks[0].size_bps = 10_000u16.into();
    profile.asks[0].expiry_offset = u32::MAX.into();
    let mut profile_bytes = [0u8; 2 * N_LEVELS * 10];
    profile_bytes.copy_from_slice(anchor_lang_v2::bytemuck::bytes_of(&profile));
    let ix = Instruction::new_with_bytes(
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
    send_ixn(&mut svm, &authority, ix).expect("set_liquidity_profile");

    // Seed via deposit_leader.
    let base_amount = 1_000_000_u64;
    let quote_amount = 1_085_000_u64;
    let leader_base = create_associated_token_account(
        &mut svm,
        &authority,
        &authority.pubkey(),
        &base_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    let leader_quote = create_associated_token_account(
        &mut svm,
        &authority,
        &authority.pubkey(),
        &quote_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(&mut svm, &authority, &base_mint, &leader_base, base_amount);
    mint_to(&mut svm, &authority, &quote_mint, &leader_quote, quote_amount);
    let ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &DepositLeaderInstruction {
            vault_idx: 0,
            base_in: base_amount,
            quote_in: quote_amount,
            max_base_in: base_amount,
            max_quote_in: quote_amount,
        }
        .data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new_readonly(quote_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(leader_base, false),
            AccountMeta::new(leader_quote, false),
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(event_authority(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, ix).expect("deposit_leader");

    (
        svm,
        authority,
        market,
        base_mint,
        quote_mint,
        base_treasury,
        quote_treasury,
    )
}

fn fund_taker(
    svm: &mut anchor_v2_testing::LiteSVM,
    authority: &Keypair,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_amount: u64,
    quote_amount: u64,
) -> (Keypair, Pubkey, Pubkey) {
    let taker = Keypair::new();
    svm.airdrop(&taker.pubkey(), 10 * SIGNER_FUNDING_LAMPORTS)
        .unwrap();
    let t_base = create_associated_token_account(
        svm,
        authority,
        &taker.pubkey(),
        base_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    let t_quote = create_associated_token_account(
        svm,
        authority,
        &taker.pubkey(),
        quote_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    if base_amount > 0 {
        mint_to(svm, authority, base_mint, &t_base, base_amount);
    }
    if quote_amount > 0 {
        mint_to(svm, authority, quote_mint, &t_quote, quote_amount);
    }
    (taker, t_base, t_quote)
}

#[allow(clippy::too_many_arguments)]
fn swap_ix(
    taker: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    t_base: &Pubkey,
    t_quote: &Pubkey,
    side: u8,
    amount_in: u64,
    limit_price_bits: u32,
    min_out: u64,
) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &SwapInstruction {
            side,
            amount_in,
            limit_price_bits,
            min_out,
        }
        .data(),
        vec![
            AccountMeta::new(*taker, true),
            AccountMeta::new(*market, false),
            AccountMeta::new_readonly(*base_mint, false),
            AccountMeta::new_readonly(*quote_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(*t_base, false),
            AccountMeta::new(*t_quote, false),
            AccountMeta::new(*base_treasury, false),
            AccountMeta::new(*quote_treasury, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
            AccountMeta::new_readonly(event_authority(), false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    )
}

#[test]
fn buy_fills_against_seeded_vault() {
    let (mut svm, authority, market, base_mint, quote_mint, base_treasury, quote_treasury) =
        setup_seeded_vault();
    // Taker buys base, pays quote. Funded with quote only.
    let (taker, t_base, t_quote) =
        fund_taker(&mut svm, &authority, &base_mint, &quote_mint, 0, 200_000);
    let taker_quote_before = token_balance(&svm, &t_quote);

    // Buy with INFINITY limit (no upper bound), spend 100_000 quote
    // atoms, min_out = 1 base atom (any non-zero fill is acceptable).
    let ix = swap_ix(
        &taker.pubkey(),
        &market,
        &base_mint,
        &quote_mint,
        &base_treasury,
        &quote_treasury,
        &t_base,
        &t_quote,
        0, // Buy
        100_000,
        Price::INFINITY.as_u32(),
        1,
    );
    send_ixn(&mut svm, &taker, ix).expect("swap Buy");

    // Taker received base, paid quote.
    let taker_base_after = token_balance(&svm, &t_base);
    let taker_quote_after = token_balance(&svm, &t_quote);
    assert!(taker_base_after > 0, "taker received some base");
    assert!(taker_quote_after < taker_quote_before, "taker spent quote");

    // Vault inventory shifted in the matching direction.
    let (_h, v) = read_first_vault(&svm, &market);
    assert!(
        v.base_atoms.get() < 1_000_000,
        "vault base inventory decreased after sale"
    );
    assert!(
        v.quote_atoms.get() > 1_085_000,
        "vault quote inventory increased after sale"
    );
}

#[test]
fn min_out_soft_reverts_when_unattainable() {
    let (mut svm, authority, market, base_mint, quote_mint, base_treasury, quote_treasury) =
        setup_seeded_vault();
    let (taker, t_base, t_quote) =
        fund_taker(&mut svm, &authority, &base_mint, &quote_mint, 0, 200_000);
    let taker_quote_before = token_balance(&svm, &t_quote);
    let (header_before, vault_before) = read_first_vault(&svm, &market);

    // min_out is 10× what the swap could possibly deliver — the
    // matcher must roll back every mutation and return Ok(). The
    // surrounding tx must succeed (we assert by sending it without
    // expecting an Err).
    let ix = swap_ix(
        &taker.pubkey(),
        &market,
        &base_mint,
        &quote_mint,
        &base_treasury,
        &quote_treasury,
        &t_base,
        &t_quote,
        0, // Buy
        100_000,
        Price::INFINITY.as_u32(),
        u64::MAX, // unattainable
    );
    send_ixn(&mut svm, &taker, ix).expect("soft-revert swap should still succeed");

    // Taker balances UNCHANGED — no transfers fired.
    assert_eq!(token_balance(&svm, &t_quote), taker_quote_before);
    assert_eq!(token_balance(&svm, &t_base), 0);

    // Vault inventory + market nonce restored to pre-swap.
    let (header_after, vault_after) = read_first_vault(&svm, &market);
    assert_eq!(vault_before.base_atoms.get(), vault_after.base_atoms.get());
    assert_eq!(vault_before.quote_atoms.get(), vault_after.quote_atoms.get());
    assert_eq!(header_before.nonce.get(), header_after.nonce.get());
    // Treasury invariant holds.
    assert_eq!(token_balance(&svm, &base_treasury), vault_after.base_atoms.get());
    assert_eq!(
        token_balance(&svm, &quote_treasury),
        vault_after.quote_atoms.get()
    );

    // Suppress the "unused" warning for the slab decoder.
    let registry_account = svm
        .get_account(&registry_pda())
        .expect("registry");
    let (_, _) = decode_slab::<dropset::RegistryHeader, [u8; 32]>(&registry_account.data);
}

#[test]
fn invalid_side_byte_rejects() {
    let (mut svm, authority, market, base_mint, quote_mint, base_treasury, quote_treasury) =
        setup_seeded_vault();
    let (taker, t_base, t_quote) =
        fund_taker(&mut svm, &authority, &base_mint, &quote_mint, 0, 200_000);
    // Side byte 2 — not Buy (0) or Sell (1).
    let ix = swap_ix(
        &taker.pubkey(),
        &market,
        &base_mint,
        &quote_mint,
        &base_treasury,
        &quote_treasury,
        &t_base,
        &t_quote,
        2,
        100_000,
        Price::INFINITY.as_u32(),
        0,
    );
    let err = send_ixn(&mut svm, &taker, ix)
        .expect_err("swap with side=2 must reject as InvalidSwapSide");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSwapSide);
}
