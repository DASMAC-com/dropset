//! `set_reference_price` integration tests — leader hot path. Covers
//! the happy stamp (FLUSH_BIT armed, nonce bumped), the authority /
//! frozen / sector gates, the `Price` validity + sentinel rejections,
//! and the `quote_slot` future/backdate bounds.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::{Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Bootstrap + one admin vault (sector 0) with `authority` as both
/// leader and quote authority. No reference price set yet.
fn fixture_with_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register_vault");
    f
}

fn valid_price() -> u32 {
    Price::encode(10_850_000, 0).unwrap().as_u32()
}

#[test]
fn happy_path_arms_flush_and_bumps_nonce() {
    let mut f = fixture_with_vault();
    let nonce_before = f.market_header().nonce.get();
    let signer = f.authority.insecure_clone();

    f.set_reference_price(&signer, 0, valid_price(), 0)
        .expect("quote authority sets reference price");

    let v = f.vault(0);
    assert_eq!(v.reference_price.price.as_u32(), valid_price());
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT armed on stamp"
    );
    assert_eq!(
        f.market_header().nonce.get(),
        nonce_before + 1,
        "market nonce bumped"
    );
}

#[test]
fn rejects_unauthorized_signer() {
    let mut f = fixture_with_vault();
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_reference_price(&stranger, 0, valid_price(), 0)
        .expect_err("non quote-authority signer must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn rejects_frozen_vault() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    f.freeze_vault(&signer, 0).expect("admin freezes vault");
    let err = f
        .set_reference_price(&signer, 0, valid_price(), 0)
        .expect_err("frozen vault must reject a reference-price update");
    common::assert_program_error(&err, dropset::DropsetError::VaultFrozen);
}

#[test]
fn rejects_invalid_price_bit_pattern() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    // Significand 5_000_000 is below the 8-digit minimum — not a valid
    // encoding and not a sentinel.
    let err = f
        .set_reference_price(&signer, 0, 5_000_000, 0)
        .expect_err("invalid significand must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidPrice);
}

#[test]
fn rejects_zero_and_infinity_sentinels() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    let err_zero = f
        .set_reference_price(&signer, 0, Price::ZERO.as_u32(), 0)
        .expect_err("Price::ZERO must reject");
    common::assert_program_error(&err_zero, dropset::DropsetError::InvalidPrice);

    let err_inf = f
        .set_reference_price(&signer, 0, Price::INFINITY.as_u32(), 0)
        .expect_err("Price::INFINITY must reject");
    common::assert_program_error(&err_inf, dropset::DropsetError::InvalidPrice);
}

#[test]
fn rejects_future_dated_quote_slot() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    // quote_slot far ahead of the current slot (and within u32) is
    // future-dated.
    let err = f
        .set_reference_price(&signer, 0, valid_price(), 1_000_000)
        .expect_err("future-dated quote_slot must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidQuoteSlot);
}

#[test]
fn rejects_backdated_past_max_backdate() {
    let mut f = fixture_with_vault();
    // Advance the clock so a quote_slot of 0 is > MAX_BACKDATE (50)
    // slots stale.
    f.svm.warp_to_slot(100);
    let signer = f.authority.insecure_clone();
    let err = f
        .set_reference_price(&signer, 0, valid_price(), 0)
        .expect_err("quote_slot stale past MAX_BACKDATE must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidQuoteSlot);
}

#[test]
fn rejects_out_of_range_sector() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    let err = f
        .set_reference_price(&signer, 99, valid_price(), 0)
        .expect_err("vault_idx past the slab length must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}
