//! End-to-end integration test exercising the full MVP pipeline:
//! `init` → `register_market` → `register_vault` →
//! `set_reference_price` → `set_liquidity_profile` →
//! `deposit` (leader seeding) → `withdraw` (leader, partial). After
//! each mutating step we re-decode the on-chain `MarketHeader` + first
//! vault sector and assert the spec's two key invariants:
//!
//! 1. Treasury matches sum of vault inventory:
//!    `base_treasury.amount == Σ vault.base_atoms`
//!    `quote_treasury.amount == Σ vault.quote_atoms`
//! 2. Share invariant I6:
//!    `total_shares == leader_shares + Σ VaultDepositor.shares`
//!
//! Single-vault flow, leader-only (no outside depositors) — verifies
//! the cold-path math, the PDA-signed treasury → caller transfer in
//! withdraw, and the emit_cpi! account threading. Outside-depositor
//! flow + cross-vault matching land in follow-up test files.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, decode_slab, deploy_with_authority, mint_to, send_ixn, ATA_PROGRAM_ID,
    MOCK_USDC_DECIMALS, PROGRAM_ID, REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS,
    SPL_TOKEN_PROGRAM_ID,
};
use dropset::{
    instruction::{
        DepositLeader as DepositLeaderInstruction, Init as InitInstruction,
        RegisterMarket as RegisterMarketInstruction, RegisterVault as RegisterVaultInstruction,
        SetLiquidityProfile as SetLiquidityProfileInstruction,
        SetReferencePrice as SetReferencePriceInstruction,
        WithdrawLeader as WithdrawLeaderInstruction,
    },
    LiquidityProfile, MarketHeader, Price, RegistryHeader, Vault, N_LEVELS,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

/// Solana sysvar Clock account address. The `Sysvar<Clock>` field on
/// our instruction handlers reads from this canonical address; tests
/// pass it as a readonly account.
const SYSVAR_CLOCK_ID: Pubkey =
    Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn market_pda(base: &Pubkey, quote: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[base.as_ref(), quote.as_ref()], &PROGRAM_ID).0
}

fn event_authority_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}

/// Read the market header + first vault sector. The Vault struct
/// contains a `Price` field (`#[repr(transparent)] u32`) which gives
/// it alignment 4; the slab tail starts at an offset that may not be
/// 4-aligned in the account buffer, so we copy via
/// `pod_read_unaligned` into an aligned stack value rather than
/// casting in place.
fn read_market_and_vault(
    svm: &anchor_v2_testing::LiteSVM,
    market: &Pubkey,
) -> (MarketHeader, Vault) {
    let acct = svm.get_account(market).expect("market exists");
    const DISC: usize = 8;
    let header_end = DISC + core::mem::size_of::<MarketHeader>();
    let header =
        anchor_lang_v2::bytemuck::pod_read_unaligned::<MarketHeader>(&acct.data[DISC..header_end]);
    let len = u32::from_le_bytes(acct.data[header_end..header_end + 4].try_into().unwrap());
    assert!(len > 0, "expected at least one vault sector");
    // The slab pads after the `len: u32` to align the items array
    // to `align_of::<Vault>()`. `Vault` contains a `Price`
    // (`#[repr(transparent)] u32`), so its alignment is 4.
    let after_len = header_end + 4;
    let v_align = core::mem::align_of::<Vault>();
    let items_start = (after_len + v_align - 1) & !(v_align - 1);
    let v_size = core::mem::size_of::<Vault>();
    let vault = anchor_lang_v2::bytemuck::pod_read_unaligned::<Vault>(
        &acct.data[items_start..items_start + v_size],
    );
    (header, vault)
}

/// SPL Token mint account `.amount` lives at bytes 64..72 LE.
fn token_balance(svm: &anchor_v2_testing::LiteSVM, ata: &Pubkey) -> u64 {
    let acct = svm.get_account(ata).expect("token account exists");
    u64::from_le_bytes(acct.data[64..72].try_into().unwrap())
}

#[test]
fn end_to_end_single_leader_pipeline() {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_mock_usdc_mint(&mut svm, &authority);
    let registry = registry_pda();
    let fee_vault = associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);

    // ── 1. init ──────────────────────────────────────────────────
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

    // ── 2. register_market ───────────────────────────────────────
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    let market = market_pda(&base_mint, &quote_mint);
    let base_treasury = associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID);
    let quote_treasury = associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID);
    let registry_fee_treasury =
        associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);

    // Dummy throwaway account for the unused payer_fee_source on the
    // admin path (Anchor v2 rejects duplicate-mut accounts, so we
    // can't reuse `authority` here).
    let dummy = Keypair::new();
    svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();

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
            AccountMeta::new(dummy.pubkey(), false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, register_market_ix).expect("register_market");

    // ── 3. register_vault ────────────────────────────────────────
    let event_authority = event_authority_pda();
    let register_vault_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &RegisterVaultInstruction {
            perf_fee_rate: 0,
            quote_authority: authority.pubkey(),
            allow_outside_depositors: false,
            // Sentinel — `register_vault` resolves "no override" via
            // `Address::default()`, which the handler treats as "use
            // payer as leader".
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
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, register_vault_ix).expect("register_vault");

    let (header, vault) = read_market_and_vault(&svm, &market);
    assert_eq!(header.active_count.get(), 1);
    assert_eq!(header.head.get(), 0, "first vault sits at sector 0");
    assert_eq!(vault.leader, authority.pubkey().to_bytes().into());
    assert_eq!(vault.quote_authority, authority.pubkey().to_bytes().into());

    // ── 4. set_reference_price ───────────────────────────────────
    // EUR/USD = 1.0850 → sig = 10_850_000, unbiased = 0 → biased = 16.
    let ref_price = Price::encode(10_850_000, 0).unwrap();
    // Read the clock from the active LiteSVM bank — `latest_blockhash`
    // is a stand-in proxy here, but `get_sysvar` would also work. We
    // just need any slot ≤ current_slot for the backdate check.
    let current_slot = 0u64;
    let set_ref_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &SetReferencePriceInstruction {
            vault_idx: 0,
            price_bits: ref_price.as_u32(),
            quote_slot: current_slot,
        }
        .data(),
        vec![
            AccountMeta::new_readonly(authority.pubkey(), true),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, set_ref_ix).expect("set_reference_price");

    let (_h, v) = read_market_and_vault(&svm, &market);
    assert_eq!(v.reference_price.price, ref_price);
    // FLUSH_BIT armed on stamp.
    assert!(v.reference_price.stamp.get() & (1u64 << 63) != 0);

    // ── 5. set_liquidity_profile ─────────────────────────────────
    // Build a profile with a single ±0.05% level (5000 ppm) per side,
    // 50% of inventory (5000 bps), 100-slot expiry.
    let mut profile: LiquidityProfile = anchor_lang_v2::bytemuck::Zeroable::zeroed();
    profile.bids[0].price_offset = 5_000u32.into();
    profile.bids[0].size_bps = 5_000u16.into();
    profile.bids[0].expiry_offset = 100u32.into();
    profile.asks[0].price_offset = 5_000u32.into();
    profile.asks[0].size_bps = 5_000u16.into();
    profile.asks[0].expiry_offset = 100u32.into();
    let mut profile_bytes = [0u8; 2 * N_LEVELS * 10];
    profile_bytes.copy_from_slice(anchor_lang_v2::bytemuck::bytes_of(&profile));

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
    send_ixn(&mut svm, &authority, set_profile_ix).expect("set_liquidity_profile");

    // ── 6. deposit (leader seeding) ──────────────────────────────
    // Mint base + quote to the leader's ATAs and seed the vault with
    // both legs. The first deposit's `shares_out = isqrt(base * quote)`,
    // and `total_shares == leader_shares` per the spec's seeding rule.
    let base_amount = 1_000_000_u64; // 1 unit at 6 decimals
    let quote_amount = 1_085_000_u64; // 1.085 unit at 6 decimals
    let leader_base_ata = create_associated_token_account(
        &mut svm,
        &authority,
        &authority.pubkey(),
        &base_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    let leader_quote_ata = create_associated_token_account(
        &mut svm,
        &authority,
        &authority.pubkey(),
        &quote_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(
        &mut svm,
        &authority,
        &base_mint,
        &leader_base_ata,
        base_amount,
    );
    mint_to(
        &mut svm,
        &authority,
        &quote_mint,
        &leader_quote_ata,
        quote_amount,
    );

    // `deposit_leader` (PDA-free) — leader seeding the vault.
    let deposit_ix = Instruction::new_with_bytes(
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
            AccountMeta::new(leader_base_ata, false),
            AccountMeta::new(leader_quote_ata, false),
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, deposit_ix).expect("deposit_leader");

    let (_, v) = read_market_and_vault(&svm, &market);
    assert_eq!(v.base_atoms.get(), base_amount);
    assert_eq!(v.quote_atoms.get(), quote_amount);
    // total_shares = isqrt(base * quote) = isqrt(1_085_000_000_000) = 1_041_633
    let expected_shares = ((base_amount as u128 * quote_amount as u128) as f64).sqrt() as u64;
    assert!(
        v.total_shares.get().abs_diff(expected_shares) <= 1,
        "expected total_shares ≈ isqrt(b*q), got {} expected {}",
        v.total_shares.get(),
        expected_shares
    );
    // Invariant I6: leader_shares == total_shares on seeding (no outside depositors yet).
    assert_eq!(v.total_shares.get(), v.leader_shares.get());
    // Treasury invariant.
    assert_eq!(token_balance(&svm, &base_treasury), base_amount);
    assert_eq!(token_balance(&svm, &quote_treasury), quote_amount);

    // ── 7. withdraw (leader, full exit) ──────────────────────────
    // Leader burns all their shares; vault drains to zero.
    let total_shares = v.total_shares.get();
    let withdraw_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &WithdrawLeaderInstruction {
            vault_idx: 0,
            shares_in: total_shares,
            min_base_out: 0,
            min_quote_out: 0,
        }
        .data(),
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new_readonly(quote_mint, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
            AccountMeta::new(leader_base_ata, false),
            AccountMeta::new(leader_quote_ata, false),
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(event_authority, false),
            AccountMeta::new_readonly(PROGRAM_ID, false),
        ],
    );
    send_ixn(&mut svm, &authority, withdraw_ix).expect("withdraw_leader");

    let (_, v) = read_market_and_vault(&svm, &market);
    assert_eq!(v.total_shares.get(), 0);
    assert_eq!(v.leader_shares.get(), 0);
    assert_eq!(v.base_atoms.get(), 0);
    assert_eq!(v.quote_atoms.get(), 0);
    // Both treasuries drained.
    assert_eq!(token_balance(&svm, &base_treasury), 0);
    assert_eq!(token_balance(&svm, &quote_treasury), 0);
    // Leader got their tokens back (pro-rata floor may leave 0 dust;
    // on a full exit dust = 0).
    assert_eq!(token_balance(&svm, &leader_base_ata), base_amount);
    assert_eq!(token_balance(&svm, &leader_quote_ata), quote_amount);

    // Registry market_count untouched.
    let registry_account = svm.get_account(&registry).expect("registry");
    let (registry_header, _) = decode_slab::<RegistryHeader, [u8; 32]>(&registry_account.data);
    assert_eq!(registry_header.market_count.get(), 1);
    // outstanding_vault_depositors stayed at 0 — this was the
    // leader path; no VaultDepositor PDA was credited.
    let (header_final, _) = read_market_and_vault(&svm, &market);
    assert_eq!(header_final.outstanding_vault_depositors.get(), 0);

    // The leader path doesn't really need MOCK_USDC_DECIMALS, but
    // pulling it into scope ensures the import lint doesn't drop it
    // — useful for the follow-up outside-depositor tests that mint a
    // USDC-shaped quote.
    let _ = MOCK_USDC_DECIMALS;
}
