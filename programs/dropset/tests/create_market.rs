//! Integration tests for the `create_market` instruction. Each test
//! stands up a fresh SVM, deploys the program, seats a genesis admin in
//! the registry against a mock-USDC fee mint, and exercises one slice
//! of the happy-path / failure-mode matrix.

mod common;

use anchor_lang_v2::{programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, Signer};
use common::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, create_token2022_mint, decode_slab, deploy_with_authority, mint_to, send_ixn,
    ATA_PROGRAM_ID, CREATE_MARKET_FEE_ATOMS, PROGRAM_ID, SIGNER_FUNDING_LAMPORTS,
    SPL_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID,
};
use dropset::{
    instruction::{CreateMarket as CreateMarketInstruction, Init as InitInstruction},
    MarketHeader, RegistryHeader, NULL_SECTOR, N_LEVELS,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

// ── Fixture wiring ───────────────────────────────────────────────────

/// Returns `(svm, authority, fee_mint)`. The authority is the program's
/// upgrade authority + genesis admin; the fee mint is a fresh
/// USDC-shaped SPL mint with mint authority on `authority`. The
/// registry is initialized against that fee mint with a $1,000 fee.
fn bootstrap() -> (anchor_v2_testing::LiteSVM, Keypair, Pubkey) {
    let authority = Keypair::new();
    let mut svm = deploy_with_authority(&authority);
    let fee_mint = create_mock_usdc_mint(&mut svm, &authority);

    // Init the registry. The init handler also stamps
    // `default_fee_config.{atoms, mint, token_program}` and creates
    // the registry's fee vault inline.
    let registry = registry_pda();
    let fee_vault = associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    let init_ix = Instruction::new_with_bytes(
        PROGRAM_ID,
        &InitInstruction {
            genesis_admin: authority.pubkey(),
            fee_atoms: CREATE_MARKET_FEE_ATOMS,
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
    send_ixn(&mut svm, &authority, init_ix).expect("registry init must succeed");
    (svm, authority, fee_mint)
}

fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}

fn market_pda(base_mint: &Pubkey, quote_mint: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[base_mint.as_ref(), quote_mint.as_ref()], &PROGRAM_ID)
}

/// Assemble a `CreateMarket` instruction with everything pre-derived.
/// `payer` signs the tx; `signer_fee_source` is the payer's source ATA
/// — pass any pubkey on the admin path (the handler skips reading it).
#[allow(clippy::too_many_arguments)]
fn create_market_ixn(
    payer: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    base_token_program: Pubkey,
    quote_token_program: Pubkey,
    fee_mint: Pubkey,
    fee_token_program: Pubkey,
    payer_fee_source: Pubkey,
) -> Instruction {
    let registry = registry_pda();
    let (market, _) = market_pda(&base_mint, &quote_mint);
    let base_treasury = associated_token_address(&market, &base_mint, &base_token_program);
    let quote_treasury = associated_token_address(&market, &quote_mint, &quote_token_program);
    let registry_fee_treasury = associated_token_address(&registry, &fee_mint, &fee_token_program);
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &CreateMarketInstruction {}.data(),
        // Order MUST match the `CreateMarket` derive — anchor reads
        // positionally, not by name.
        vec![
            AccountMeta::new(payer, true),
            // `mut`: create_market bumps registry.market_count.
            AccountMeta::new(registry, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new_readonly(quote_mint, false),
            AccountMeta::new_readonly(base_token_program, false),
            AccountMeta::new_readonly(quote_token_program, false),
            AccountMeta::new(market, false),
            AccountMeta::new(base_treasury, false),
            AccountMeta::new(quote_treasury, false),
            AccountMeta::new_readonly(fee_mint, false),
            AccountMeta::new_readonly(fee_token_program, false),
            AccountMeta::new(payer_fee_source, false),
            AccountMeta::new(registry_fee_treasury, false),
            AccountMeta::new_readonly(System::id(), false),
            AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
        ],
    )
}

/// Decode the market header out of the slab account bytes: skip the
/// 8-byte anchor discriminator, then bytemuck-cast.
fn read_market_header(svm: &anchor_v2_testing::LiteSVM, market: &Pubkey) -> MarketHeader {
    let account = svm.get_account(market).expect("market account exists");
    let data = &account.data;
    assert!(data.len() >= 8 + core::mem::size_of::<MarketHeader>());
    *anchor_lang_v2::bytemuck::from_bytes::<MarketHeader>(
        &data[8..8 + core::mem::size_of::<MarketHeader>()],
    )
}

/// Fund a fresh payer keypair with enough SOL to cover the per-tx fees
/// and the market + treasury rent. SIGNER_FUNDING_LAMPORTS is 1 SOL —
/// far more than enough for either.
fn fresh_payer(svm: &mut anchor_v2_testing::LiteSVM) -> Keypair {
    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 10 * SIGNER_FUNDING_LAMPORTS)
        .unwrap();
    payer
}

/// Mint `CREATE_MARKET_FEE_ATOMS` of `fee_mint` to a fresh ATA owned
/// by `wallet`, and return that ATA. Used by the non-admin path so the
/// fee-transfer CPI has the right balance.
fn fund_with_fee(
    svm: &mut anchor_v2_testing::LiteSVM,
    mint_authority: &Keypair,
    fee_mint: &Pubkey,
    wallet: &Keypair,
) -> Pubkey {
    let ata = create_associated_token_account(
        svm,
        mint_authority,
        &wallet.pubkey(),
        fee_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(svm, mint_authority, fee_mint, &ata, CREATE_MARKET_FEE_ATOMS);
    ata
}

/// Token balance of an SPL Token account: the SPL layout puts amount at
/// offset 64..72, little-endian u64.
fn token_balance(svm: &anchor_v2_testing::LiteSVM, ata: &Pubkey) -> u64 {
    let account = svm.get_account(ata).expect("token account exists");
    let bytes: [u8; 8] = account.data[64..72].try_into().unwrap();
    u64::from_le_bytes(bytes)
}

// ── Happy paths ─────────────────────────────────────────────────────

#[test]
fn create_market_succeeds_with_two_spl_mints() {
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let payer_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer);
    // Two fresh SPL mints with `authority` as mint authority.
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);

    let ix = create_market_ixn(
        payer.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix).expect("create_market should succeed");

    // Market header is stamped with the values seeded from the registry,
    // and both list heads start empty.
    let (market, market_bump) = market_pda(&base_mint, &quote_mint);
    let header = read_market_header(&svm, &market);
    assert_eq!(header.base_mint, base_mint.to_bytes().into());
    assert_eq!(header.quote_mint, quote_mint.to_bytes().into());
    assert_eq!(header.bump, market_bump);
    assert_eq!(header.head.get(), NULL_SECTOR);
    assert_eq!(header.tombstone_head.get(), NULL_SECTOR);
    assert_eq!(header.free_head.get(), NULL_SECTOR);
    assert_eq!(header.active_count.get(), 0);
    // Fresh market has no depositors — the counter only advances when
    // an outside Deposit opens a fresh VaultDepositor PDA.
    assert_eq!(header.outstanding_vault_depositors.get(), 0);
    assert_eq!(header.nonce.get(), 0);
    assert_eq!(header.fee_config.mint, fee_mint.to_bytes().into());
    assert_eq!(header.fee_config.atoms.get(), CREATE_MARKET_FEE_ATOMS);

    // Registry bumped the live-market counter — the on-chain witness
    // `close_registry` checks against under the `admin-teardown`
    // feature. See architecture spec, **Account lifecycle and rent
    // reclamation**.
    let registry_account = svm
        .get_account(&registry_pda())
        .expect("registry account exists");
    let (registry_header, _) = decode_slab::<RegistryHeader, [u8; 32]>(&registry_account.data);
    assert_eq!(
        registry_header.market_count.get(),
        1,
        "create_market should increment market_count from 0 to 1"
    );

    // Treasuries are the canonical ATAs at `market` for each mint.
    assert_eq!(
        header.base_treasury,
        associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID)
            .to_bytes()
            .into()
    );
    assert_eq!(
        header.quote_treasury,
        associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID)
            .to_bytes()
            .into()
    );

    // The fee actually moved.
    assert_eq!(token_balance(&svm, &payer_ata), 0);
    let registry_fee_ata =
        associated_token_address(&registry_pda(), &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    assert_eq!(
        token_balance(&svm, &registry_fee_ata),
        CREATE_MARKET_FEE_ATOMS
    );

    // Vault tail starts empty: the slab `len` field sits right after
    // the header at offset 8 + size_of::<MarketHeader>().
    let account = svm.get_account(&market).expect("market exists");
    let len_off = 8 + core::mem::size_of::<MarketHeader>();
    let len = u32::from_le_bytes(account.data[len_off..len_off + 4].try_into().unwrap());
    assert_eq!(len, 0, "no vaults at create_market time");

    // N_LEVELS hasn't drifted unintentionally — surface that here so
    // any vault-layout retune lands as a deliberate test update too.
    assert_eq!(N_LEVELS, 8);
}

#[test]
fn create_market_succeeds_with_mixed_token_programs() {
    // Real FX pair shape: classic SPL on one leg, Token-2022 on the
    // other. The ATA program derives different addresses per program,
    // so the test exercises both derivation paths in one call.
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let payer_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer);
    let base_mint = create_spl_mint(&mut svm, &authority); // SPL
    let quote_mint = create_token2022_mint(&mut svm, &authority); // Token-2022

    let ix = create_market_ixn(
        payer.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        TOKEN_2022_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix).expect("mixed-program create_market should succeed");

    let (market, _) = market_pda(&base_mint, &quote_mint);
    let header = read_market_header(&svm, &market);
    // The base treasury is derived under SPL Token, the quote under
    // Token-2022 — distinct addresses for the same wallet/mint pair.
    assert_eq!(
        header.base_treasury,
        associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID)
            .to_bytes()
            .into()
    );
    assert_eq!(
        header.quote_treasury,
        associated_token_address(&market, &quote_mint, &TOKEN_2022_PROGRAM_ID)
            .to_bytes()
            .into()
    );
}

#[test]
fn admin_path_waives_fee() {
    // The genesis admin (`authority`) is on the admin allowlist, so the
    // fee transfer is skipped. They don't even need to hold the fee
    // mint — any throwaway pubkey works as `payer_fee_source` since the
    // handler's admin branch never reads it.
    let (mut svm, authority, fee_mint) = bootstrap();
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);

    // Fresh throwaway pubkey — distinct from `payer` so anchor's
    // duplicate-mutable-account check doesn't trip before our handler
    // runs. Funded with lamports so the runtime recognizes it as an
    // existing system account.
    let dummy_source = Keypair::new();
    svm.airdrop(&dummy_source.pubkey(), SIGNER_FUNDING_LAMPORTS)
        .unwrap();

    let ix = create_market_ixn(
        authority.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        dummy_source.pubkey(),
    );
    send_ixn(&mut svm, &authority, ix).expect("admin path should waive fee");

    // Registry fee ATA exists (init_if_needed ran) but holds zero.
    let registry_fee_ata =
        associated_token_address(&registry_pda(), &fee_mint, &SPL_TOKEN_PROGRAM_ID);
    assert_eq!(token_balance(&svm, &registry_fee_ata), 0);

    // The admin path still bumps `market_count` — the increment lives
    // after the fee branch, so both paths share it.
    let registry_account = svm
        .get_account(&registry_pda())
        .expect("registry account exists");
    let (registry_header, _) = decode_slab::<RegistryHeader, [u8; 32]>(&registry_account.data);
    assert_eq!(registry_header.market_count.get(), 1);
}

// ── Failure modes ───────────────────────────────────────────────────

#[test]
fn rejects_same_mint_for_base_and_quote() {
    // A market against `(mint, mint)` would yield two identical
    // treasury ATAs (same wallet, mint, token program) — both written
    // by the same instruction. Anchor's per-tx duplicate-mutable check
    // (`ConstraintDuplicateMutableAccount`, custom code 2005) fires
    // before our handler runs, so the in-handler
    // `DuplicateBaseQuoteMint` check is a backstop rather than the
    // first line of defense. The point of this test is that the
    // single-mint case is rejected, not which layer rejects it.
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let payer_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer);
    let mint = create_spl_mint(&mut svm, &authority);

    let ix = create_market_ixn(
        payer.pubkey(),
        mint,
        mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix).expect_err("same-mint market must be rejected");
}

#[test]
fn rejects_wrong_fee_mint() {
    let (mut svm, authority, _fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    // A different mint, not the one the registry was initialized with.
    let bogus_fee_mint = create_spl_mint(&mut svm, &authority);
    // Caller's source ATA in the *bogus* mint (also funded so we hit
    // the in-handler fee-mint check, not a pre-flight transfer fail).
    let bogus_source = common::create_associated_token_account(
        &mut svm,
        &authority,
        &payer.pubkey(),
        &bogus_fee_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(
        &mut svm,
        &authority,
        &bogus_fee_mint,
        &bogus_source,
        CREATE_MARKET_FEE_ATOMS,
    );

    // The `address = registry.default_fee_config.mint` constraint on
    // `fee_mint` will fail here. Anchor v2 loads accounts before
    // checking constraints, so a non-existent ATA derived from the
    // bogus mint surfaces as `AccountDataTooSmall` at load time —
    // either way the wrong mint is rejected. The point of this test
    // is the rejection, not the specific code.
    let ix = create_market_ixn(
        payer.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        bogus_fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        bogus_source,
    );
    send_ixn(&mut svm, &payer, ix).expect_err("wrong fee mint must be rejected");
}

#[test]
fn rejects_underfunded_payer() {
    // Non-admin payer without enough mock-USDC to cover the fee. The
    // `transfer_checked` CPI to the token program returns
    // `InsufficientFunds` (`ProgramError::Custom(1)`); we just assert
    // it errors — any program-level failure is enough.
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    // Create the ATA but fund less than the fee.
    let payer_ata = create_associated_token_account(
        &mut svm,
        &authority,
        &payer.pubkey(),
        &fee_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );
    mint_to(
        &mut svm,
        &authority,
        &fee_mint,
        &payer_ata,
        CREATE_MARKET_FEE_ATOMS - 1,
    );
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);

    let ix = create_market_ixn(
        payer.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix).expect_err("underfunded fee transfer must fail");
}

#[test]
fn rejects_second_create_market_on_same_pair() {
    // The `init` constraint enforces single-shot creation — the runtime
    // refuses to re-create an existing PDA, so the second call fails
    // before our handler runs.
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let payer_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer);
    let base_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);

    let ix = create_market_ixn(
        payer.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix.clone()).expect("first create_market must succeed");

    // Fund a fresh ATA so the second attempt fails at `init`, not at
    // the source balance.
    let payer2 = fresh_payer(&mut svm);
    let payer2_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer2);
    let ix2 = create_market_ixn(
        payer2.pubkey(),
        base_mint,
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer2_ata,
    );
    send_ixn(&mut svm, &payer2, ix2).expect_err("second create_market on same pair must fail");
}

#[test]
fn rejects_non_mint_as_base() {
    // A token account isn't a mint; `InterfaceAccount<Mint>` rejects on
    // load before the handler runs.
    let (mut svm, authority, fee_mint) = bootstrap();
    let payer = fresh_payer(&mut svm);
    let payer_ata = fund_with_fee(&mut svm, &authority, &fee_mint, &payer);
    let real_mint = create_spl_mint(&mut svm, &authority);
    let quote_mint = create_spl_mint(&mut svm, &authority);
    // Build a regular SPL token account against `real_mint`; this is a
    // 165-byte account owned by the SPL Token program — wrong shape for
    // a mint.
    let token_account = create_associated_token_account(
        &mut svm,
        &authority,
        &payer.pubkey(),
        &real_mint,
        &SPL_TOKEN_PROGRAM_ID,
    );

    let ix = create_market_ixn(
        payer.pubkey(),
        token_account, // bogus base "mint"
        quote_mint,
        SPL_TOKEN_PROGRAM_ID,
        SPL_TOKEN_PROGRAM_ID,
        fee_mint,
        SPL_TOKEN_PROGRAM_ID,
        payer_ata,
    );
    send_ixn(&mut svm, &payer, ix).expect_err("token account passed as mint must be rejected");
}
