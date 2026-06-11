//! Swap integration tests — exercise the multi-vault heap matcher +
//! `min_out` soft-revert behavior end-to-end against the deployed
//! `.so`. Each test builds the leader pipeline (`init` →
//! `register_market` → `register_vault` → `set_reference_price` →
//! `set_liquidity_profile` → seed via `deposit_leader`), then runs a
//! `swap` against the seeded vault and asserts the outcome.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
use common::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, decode_slab, deploy_with_authority, mint_to, send_ixn, ATA_PROGRAM_ID,
    PROGRAM_ID, REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID,
};
use dropset::FLUSH_BIT;
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
    let header =
        anchor_lang_v2::bytemuck::pod_read_unaligned::<MarketHeader>(&acct.data[DISC..header_end]);
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
    svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();
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
    mint_to(
        &mut svm,
        &authority,
        &quote_mint,
        &leader_quote,
        quote_amount,
    );
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
    assert_eq!(
        vault_before.quote_atoms.get(),
        vault_after.quote_atoms.get()
    );
    assert_eq!(header_before.nonce.get(), header_after.nonce.get());
    // Treasury invariant holds.
    assert_eq!(
        token_balance(&svm, &base_treasury),
        vault_after.base_atoms.get()
    );
    assert_eq!(
        token_balance(&svm, &quote_treasury),
        vault_after.quote_atoms.get()
    );

    // Suppress the "unused" warning for the slab decoder.
    let registry_account = svm.get_account(&registry_pda()).expect("registry");
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

// ── Fixture-based expansion (WI2) ────────────────────────────────────
//
// These reuse the shared `Fixture::seeded` bootstrap (1.0850 ref price,
// ±0.5% full-inventory ladder, seeded 1_000_000 base / 1_085_000 quote)
// rather than the bespoke `setup_seeded_vault` above.

/// Default seed used across the fixture-based swap tests.
const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

#[test]
fn sell_side_fills_against_bids() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Taker sells base, receives quote — exercises the bid-side heap
    // key + sort. Funded with base only.
    let taker = f.funded_depositor(100_000, 0);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);

    // Sell with ZERO limit (no lower bound on bid price), min_out = 1.
    f.swap(&taker, 1, 100_000, Price::ZERO.as_u32(), 1)
        .expect("sell fills");

    assert!(
        f.token_balance(&quote_ata) > 0,
        "taker received quote for the base sold"
    );
    assert!(f.token_balance(&base_ata) < base_before, "taker spent base");
    let v = f.vault(0);
    assert!(
        v.base_atoms.get() > SEED_BASE,
        "vault base grew on the buy-from-taker"
    );
    assert!(v.quote_atoms.get() < SEED_QUOTE, "vault quote shrank");
}

#[test]
fn limit_price_stops_before_level() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    // Ask sits at ~1.0904 (1.0850 × 1.005). A Buy limit of 1.08 is
    // strictly tighter, so the best (only) level crosses and nothing
    // fills — with min_out = 0 the handler returns Ok with no transfer.
    let tight_limit = Price::encode(10_800_000, 0).unwrap();
    f.swap(&taker, 0, 100_000, tight_limit.as_u32(), 0)
        .expect("swap returns Ok with no fill");

    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    assert_eq!(
        f.token_balance(&f.base_ata(&taker.pubkey())),
        0,
        "no base received"
    );
}

#[test]
fn frozen_vault_skipped_from_matching() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    // Freeze the only vault — it must drop out of the matching set even
    // though its levels would otherwise be the best (only) price.
    f.poke_frozen(0, true);
    let taker = f.funded_depositor(0, 200_000);
    let quote_ata = f.quote_ata(&taker.pubkey());
    let q_before = f.token_balance(&quote_ata);

    f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, no fill against a frozen vault");

    assert_eq!(f.token_balance(&quote_ata), q_before, "no quote spent");
    let v = f.vault(0);
    assert_eq!(
        v.base_atoms.get(),
        SEED_BASE,
        "frozen vault inventory untouched"
    );
    assert_eq!(
        v.quote_atoms.get(),
        SEED_QUOTE,
        "frozen vault inventory untouched"
    );
}

#[test]
fn min_out_boundary_commits_at_equal_and_reverts_one_over() {
    // Probe the achievable net output on a throwaway fixture.
    let achievable = {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("probe swap");
        f.token_balance(&f.base_ata(&taker.pubkey()))
    };
    assert!(achievable > 0, "probe must fill something");

    // min_out exactly equal to achievable → commits.
    {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), achievable)
            .expect("min_out == achievable commits");
        assert_eq!(f.token_balance(&f.base_ata(&taker.pubkey())), achievable);
    }
    // min_out one atom over → soft-reverts (Ok, no transfer).
    {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), achievable + 1)
            .expect("min_out one over soft-reverts");
        assert_eq!(f.token_balance(&f.base_ata(&taker.pubkey())), 0);
    }
}

#[test]
fn taker_fee_retained_in_vault() {
    // Same swap with and without a taker fee — the fee'd taker receives
    // strictly less base, and the difference stays in the vault.
    let run = |fee_ppm: u16| {
        let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
        f.poke_taker_fee(fee_ppm);
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("swap");
        (
            f.token_balance(&f.base_ata(&taker.pubkey())),
            f.vault(0).base_atoms.get(),
        )
    };
    let (no_fee_recv, _) = run(0);
    let (fee_recv, fee_vault_base) = run(10_000); // 1%

    assert!(
        fee_recv < no_fee_recv,
        "a positive taker fee leaves the taker with less base ({fee_recv} vs {no_fee_recv})"
    );
    // Without a fee the vault sent out `no_fee_recv` base; with the fee
    // it retains the fee slice, so its remaining base is higher.
    assert!(
        fee_vault_base > SEED_BASE - no_fee_recv,
        "vault retained the taker fee"
    );
}

// ── Multi-vault price-time priority + flush / expiry (WI2) ───────────

/// Build a two-vault market (sectors 0 then 1), each seeded with
/// 1_000_000 base / 1_085_000 quote and a full-inventory ±0.5% ladder,
/// but anchored at the given reference prices. Sector 0 is set up
/// first, so its `reference_price.stamp` nonce is strictly older than
/// sector 1's — the price-time tiebreaker when prices are equal.
fn seeded_two_vaults(ref0_bits: u32, ref1_bits: u32) -> Fixture {
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();

    // Sector 0.
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register vault 0");
    f.set_reference_price(&auth, 0, ref0_bits, 0)
        .expect("ref 0");
    f.set_liquidity_profile(&auth, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("profile 0");
    f.deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed 0");

    // Advance the blockhash so sector 1's seed mint (same amount, same
    // ATA as sector 0's) isn't a byte-identical, already-processed txn.
    f.svm.expire_blockhash();

    // Sector 1. Its register / seed txns mirror sector 0's argument-for
    // -argument; the blockhash bump above is what keeps them from
    // colliding as already-processed duplicates.
    f.register_vault(1, f.authority.pubkey(), false, Pubkey::default())
        .expect("register vault 1");
    f.set_reference_price(&auth, 1, ref1_bits, 0)
        .expect("ref 1");
    f.set_liquidity_profile(&auth, 1, simple_profile(5_000, 10_000, u32::MAX))
        .expect("profile 1");
    f.deposit_leader(1, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed 1");
    f
}

#[test]
fn multi_vault_cheaper_price_fills_first() {
    // Sector 1 quotes the lower reference (cheaper asks for a Buy), so
    // a small Buy must fill entirely against sector 1 and leave the
    // pricier sector 0 untouched.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = seeded_two_vaults(hi, lo);

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("buy fills the cheaper vault");

    // The cheaper vault absorbs the buy. Integer rounding can leave a
    // 1-2 atom quote residual that buys a single base atom off the
    // pricier vault's level, so assert the *bulk* landed on sector 1
    // and sector 0 saw at most rounding dust.
    let fill_cheaper = 1_000_000 - f.vault(1).base_atoms.get();
    let fill_pricier = 1_000_000 - f.vault(0).base_atoms.get();
    assert!(
        fill_cheaper >= 40_000,
        "cheaper vault (sector 1) absorbed the buy (filled {fill_cheaper})"
    );
    assert!(
        fill_pricier <= 2,
        "pricier vault (sector 0) saw only rounding dust (filled {fill_pricier})"
    );
}

#[test]
fn multi_vault_equal_price_older_nonce_wins() {
    // Both vaults anchored at the same 1.0850 reference → identical ask
    // price, so the fill order is decided purely by the price-time
    // tiebreak `(price_key, nonce, sector_idx, …)`. To isolate the
    // *nonce* term from the *sector_idx* term, give the OLDER nonce to
    // the HIGHER-index vault: `seeded_two_vaults` quotes sector 0 first,
    // so re-quote it here to stamp it with the newest nonce. Now sector
    // 1 holds the older nonce but the higher index — if `nonce` breaks
    // the tie, sector 1 fills; if `sector_idx` did, sector 0 would.
    let same = Price::encode(10_850_000, 0).unwrap().as_u32();
    let mut f = seeded_two_vaults(same, same);
    // Distinct blockhash so the re-quote isn't a duplicate of sector 0's
    // original set_reference_price txn.
    f.svm.expire_blockhash();
    f.set_reference_price(&f.authority.insecure_clone(), 0, same, 0)
        .expect("re-quote sector 0 with the newest nonce");

    let taker = f.funded_depositor(0, 200_000);
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("buy fills the older-nonce vault");

    // Sector 1 (older nonce, higher index) absorbs the buy; sector 0
    // (newer nonce, lower index) sees at most a rounding-dust atom.
    let fill_older = 1_000_000 - f.vault(1).base_atoms.get();
    let fill_newer = 1_000_000 - f.vault(0).base_atoms.get();
    assert!(
        fill_older >= 40_000,
        "older-nonce vault (sector 1) filled first (filled {fill_older})"
    );
    assert!(
        fill_newer <= 2,
        "newer-nonce vault (sector 0) untouched on the tie (filled {fill_newer})"
    );
}

#[test]
fn multi_vault_spills_cheaper_then_pricier() {
    // A Buy large enough to exhaust the cheaper vault's ask level then
    // spill into the pricier one. The cheaper sector 1 (full ask = its
    // whole 1_000_000 base) drains to zero before sector 0 is touched.
    let hi = Price::encode(10_900_000, 0).unwrap().as_u32();
    let lo = Price::encode(10_800_000, 0).unwrap().as_u32();
    let mut f = seeded_two_vaults(hi, lo);

    // 1_500_000 quote fully drains the cheaper vault's 1_000_000-base
    // ask (~1.0854e6 quote) and spills the rest into the pricier one,
    // leaving sector 0 partially filled.
    let taker = f.funded_depositor(0, 1_500_000);
    f.swap(&taker, 0, 1_500_000, Price::INFINITY.as_u32(), 1)
        .expect("large buy spills across both vaults");

    let v0 = f.vault(0).base_atoms.get();
    let v1 = f.vault(1).base_atoms.get();
    assert_eq!(v1, 0, "cheaper vault drained first");
    assert!(
        v0 < 1_000_000,
        "pricier vault partially filled by the spillover"
    );
    assert!(v0 > 0, "pricier vault not fully drained");
    assert!(
        v1 < v0,
        "cheaper vault is more depleted than the pricier one"
    );
}

#[test]
fn flush_rematerializes_after_reference_price_change() {
    // First Buy materializes `remaining` from the 1.0850 ladder. After
    // a much higher reference is stamped (re-arming FLUSH_BIT), an
    // identical Buy must re-materialize at the new (worse-for-taker)
    // ask and return less base.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let t1 = f.funded_depositor(0, 200_000);
    f.swap(&t1, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("first buy");
    let got1 = f.token_balance(&f.base_ata(&t1.pubkey()));

    let higher = Price::encode(13_000_000, 0).unwrap(); // 1.30
    f.set_reference_price(&f.authority.insecure_clone(), 0, higher.as_u32(), 0)
        .expect("raise reference, re-arms FLUSH_BIT");

    let t2 = f.funded_depositor(0, 200_000);
    f.swap(&t2, 0, 50_000, Price::INFINITY.as_u32(), 1)
        .expect("second buy at the new price");
    let got2 = f.token_balance(&f.base_ata(&t2.pubkey()));

    assert!(
        got2 < got1,
        "higher reference re-materialized a worse ask: {got2} < {got1}"
    );
}

#[test]
fn expired_levels_are_skipped() {
    // Re-profile the seeded vault with a 1-slot expiry, warp well past
    // it, then Buy: every level has expired, so nothing fills.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    f.set_liquidity_profile(
        &f.authority.insecure_clone(),
        0,
        simple_profile(5_000, 10_000, 1),
    )
    .expect("short-expiry profile");
    f.svm.warp_to_slot(100);

    let taker = f.funded_depositor(0, 200_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("ok, all levels expired");

    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "no quote spent against expired levels"
    );
    assert_eq!(
        f.vault(0).base_atoms.get(),
        SEED_BASE,
        "inventory untouched"
    );
}

/// Two ask levels, 5_000 bps each (Σ = 10_000), at different offsets.
fn two_ask_level_profile() -> [u8; PROFILE_BYTES] {
    let mut p: dropset::LiquidityProfile = anchor_lang_v2::bytemuck::Zeroable::zeroed();
    p.asks[0].price_offset = 5_000u32.into();
    p.asks[0].size_bps = 5_000u16.into();
    p.asks[0].expiry_offset = u32::MAX.into();
    p.asks[1].price_offset = 10_000u32.into();
    p.asks[1].size_bps = 5_000u16.into();
    p.asks[1].expiry_offset = u32::MAX.into();
    let mut bytes = [0u8; PROFILE_BYTES];
    bytes.copy_from_slice(anchor_lang_v2::bytemuck::bytes_of(&p));
    bytes
}

#[test]
fn min_out_soft_revert_restores_multiple_legs_and_rearms_flush() {
    // A vault with two ask levels. A Buy that crosses both, then fails
    // its `min_out`, must restore *both* levels' remaining size and
    // re-arm FLUSH_BIT.
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register");
    f.set_reference_price(&auth, 0, Price::encode(10_850_000, 0).unwrap().as_u32(), 0)
        .expect("ref");
    f.set_liquidity_profile(&auth, 0, two_ask_level_profile())
        .expect("two-level profile");
    f.deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");

    let taker = f.funded_depositor(0, 5_000_000);
    let q_before = f.token_balance(&f.quote_ata(&taker.pubkey()));
    // Big enough to fill both 500_000-base levels; min_out unattainable.
    f.swap(&taker, 0, 2_000_000, Price::INFINITY.as_u32(), u64::MAX)
        .expect("soft-revert returns Ok");

    let v = f.vault(0);
    // Each level materialized to base_atoms * 5_000 / 10_000 = 500_000;
    // the revert must restore both to that full size.
    assert_eq!(v.remaining.asks[0].size.get(), 500_000, "level 0 restored");
    assert_eq!(v.remaining.asks[1].size.get(), 500_000, "level 1 restored");
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT re-armed after soft-revert"
    );
    assert_eq!(
        f.token_balance(&f.quote_ata(&taker.pubkey())),
        q_before,
        "taker spent nothing on the reverted swap"
    );
    assert_eq!(v.base_atoms.get(), 1_000_000, "vault inventory restored");
}
