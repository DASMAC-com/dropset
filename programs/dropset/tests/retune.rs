//! Integration tests for the post-create admin retuning levers
//! (`set_min_leader_share`, `set_market_fee_config`).

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use common::{create_spl_mint, SPL_TOKEN_PROGRAM_ID, TOKEN_2022_PROGRAM_ID};
use solana_pubkey::Pubkey;

/// Bootstrap + one admin vault on sector 0 (leader + quote authority both
/// `f.authority`). The default `min_leader_share` is the 5% market floor.
fn fixture_with_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    f
}

// ── set_min_leader_share (admin-only) ────────────────────────────────

#[test]
fn min_leader_share_admin_retunes_floor() {
    let mut f = fixture_with_vault();
    // Seeded from the 5% registry/market default.
    assert_eq!(f.vault(0).min_leader_share.get(), 50_000);

    let admin = f.authority.insecure_clone();
    let meta = f
        .set_min_leader_share_meta(&admin, 0, 100_000)
        .expect("admin may retune the floor");
    assert_eq!(f.vault(0).min_leader_share.get(), 100_000);

    // The event mirrors the write.
    let ev = common::events::set_min_leader_share(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.min_leader_share, 100_000);
}

#[test]
fn min_leader_share_allows_full_floor() {
    // Exactly 100% (`PPM`) is a legitimate leader-only book, not a misuse.
    let mut f = fixture_with_vault();
    let admin = f.authority.insecure_clone();
    f.set_min_leader_share(&admin, 0, 1_000_000)
        .expect("a 100% floor is allowed");
    assert_eq!(f.vault(0).min_leader_share.get(), 1_000_000);
}

#[test]
fn min_leader_share_rejects_above_ppm() {
    let mut f = fixture_with_vault();
    let admin = f.authority.insecure_clone();
    let err = f
        .set_min_leader_share(&admin, 0, 1_000_001)
        .expect_err("a floor above 100% is unsatisfiable");
    common::assert_program_error(&err, dropset::DropsetError::InvalidMinLeaderShare);
    // The store is left untouched.
    assert_eq!(f.vault(0).min_leader_share.get(), 50_000);
}

#[test]
fn min_leader_share_retunes_frozen_vault() {
    // A frozen vault bypasses the floor for the leader and rejects all
    // outside deposits, so the stored value is inert there — but the
    // setter only gates on sector liveness (`leader != default`), not
    // the frozen flag, so retuning a frozen vault still succeeds. Pin
    // that: freezing is not a precondition the setter checks.
    let mut f = fixture_with_vault();
    let admin = f.authority.insecure_clone();
    f.freeze_vault(&admin, 0).expect("admin freezes the vault");
    assert!(f.vault(0).frozen.get());

    f.set_min_leader_share(&admin, 0, 100_000)
        .expect("a frozen vault still accepts a floor retune");
    assert_eq!(f.vault(0).min_leader_share.get(), 100_000);
}

#[test]
fn min_leader_share_rejects_non_admin() {
    let mut f = fixture_with_vault();
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_min_leader_share(&stranger, 0, 100_000)
        .expect_err("non-admin must not retune the floor");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(f.vault(0).min_leader_share.get(), 50_000);
}

#[test]
fn min_leader_share_rejects_invalid_idx() {
    let mut f = fixture_with_vault();
    let admin = f.authority.insecure_clone();
    let err = f
        .set_min_leader_share(&admin, 99, 100_000)
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}

#[test]
fn min_leader_share_rejects_empty_sector() {
    let mut f = fixture_with_vault();
    f.poke_leader_empty(0);
    let admin = f.authority.insecure_clone();
    let err = f
        .set_min_leader_share(&admin, 0, 100_000)
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, dropset::DropsetError::VaultEmpty);
}

// ── set_market_fee_config (admin-only) ───────────────────────────────

#[test]
fn market_fee_config_admin_retunes_fee() {
    let mut f = Fixture::bootstrap();
    // Seeded from the registry default at market creation.
    let before = f.market_header().fee_config;
    assert_eq!(before.mint, f.fee_mint.to_bytes().into());
    assert_eq!(before.token_program, SPL_TOKEN_PROGRAM_ID.to_bytes().into());

    // Point the create-vault fee at a fresh mint, with a new amount.
    let admin = f.authority.insecure_clone();
    let new_mint = create_spl_mint(&mut f.svm, &admin);
    let meta = f
        .set_market_fee_config_meta(&admin, &new_mint, &SPL_TOKEN_PROGRAM_ID, 42_000)
        .expect("admin may retune the market fee");

    let after = f.market_header().fee_config;
    assert_eq!(after.mint, new_mint.to_bytes().into());
    assert_eq!(after.token_program, SPL_TOKEN_PROGRAM_ID.to_bytes().into());
    assert_eq!(after.atoms.get(), 42_000);

    let ev = common::events::set_market_fee_config(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.mint, new_mint.to_bytes());
    assert_eq!(ev.token_program, SPL_TOKEN_PROGRAM_ID.to_bytes());
    assert_eq!(ev.atoms, 42_000);
}

#[test]
fn market_fee_config_rejects_non_admin() {
    let mut f = Fixture::bootstrap();
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let mint = f.fee_mint;
    let err = f
        .set_market_fee_config(&stranger, &mint, &SPL_TOKEN_PROGRAM_ID, 42_000)
        .expect_err("non-admin must not retune the market fee");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    // The config is untouched.
    assert_eq!(f.market_header().fee_config.mint, mint.to_bytes().into());
}

#[test]
fn market_fee_config_rejects_mint_program_mismatch() {
    // `fee_mint` is an SPL Token mint; passing the Token-2022 program as
    // its owner must fail the `mint::token_program` constraint before any
    // write lands.
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let mint = f.fee_mint;
    let err = f
        .set_market_fee_config(&admin, &mint, &TOKEN_2022_PROGRAM_ID, 42_000)
        .expect_err("a mint/token-program mismatch must reject");
    common::assert_instruction_error(&err, "IllegalOwner");
    assert_eq!(
        f.market_header().fee_config.token_program,
        SPL_TOKEN_PROGRAM_ID.to_bytes().into()
    );
}
