//! `set_reference_price` integration tests — leader hot path. Covers the
//! happy stamp (FLUSH_BIT armed, nonce bumped), the authority and sector
//! gates (the only checks the simplified handler keeps), and the inputs
//! the write deliberately does not validate: an invalid / sentinel price, a
//! future- or back-dated `quote_slot`, and a frozen vault are all stored
//! raw rather than rejected (per the architecture spec's
//! **SetReferencePrice** — matching skips an invalid-price or frozen
//! vault, so there is nothing to guard at write time).

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::{Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Bootstrap + one admin vault (sector 0) with `authority` as both
/// leader and quote authority. No reference price set yet.
fn fixture_with_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
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

    f.set_reference_price(&signer, 0, valid_price(), 7)
        .expect("quote authority sets reference price");

    let v = f.vault(0);
    assert_eq!(v.reference_price.price.as_u32(), valid_price());
    assert_eq!(v.reference_price.quote_slot.get(), 7, "quote_slot stored");
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "FLUSH_BIT armed on stamp"
    );
    assert_eq!(
        v.reference_price.stamp.get() & !FLUSH_BIT,
        nonce_before,
        "stamp carries the pre-bump nonce"
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
fn rejects_out_of_range_sector() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    let err = f
        .set_reference_price(&signer, 99, valid_price(), 0)
        .expect_err("vault_idx past the slab length must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}

/// The write no longer validates the price: an invalid significand and
/// both the `ZERO` / `INFINITY` sentinels are stored verbatim. Matching
/// skips a vault whose reference price isn't valid, so the blast radius is
/// a self-inflicted non-quoting vault, not a fund risk.
#[test]
fn stores_invalid_and_sentinel_prices_raw() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();

    // Significand 5_000_000 is below the 8-digit minimum — not a valid
    // encoding and not a sentinel.
    for bits in [5_000_000, Price::ZERO.as_u32(), Price::INFINITY.as_u32()] {
        f.set_reference_price(&signer, 0, bits, 0)
            .expect("invalid / sentinel price is stored, not rejected");
        assert_eq!(f.vault(0).reference_price.price.as_u32(), bits);
    }
}

/// `quote_slot` is stored raw with no clock comparison — future- and
/// back-dated values are both accepted (a stale or future slot only
/// shortens or extends the leader's own levels' liveness: self-grief, not
/// an exploit; match-time expiry is the enforcement point).
#[test]
fn stores_future_and_backdated_quote_slot() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();

    // Far ahead of the current slot.
    f.set_reference_price(&signer, 0, valid_price(), 1_000_000)
        .expect("future-dated quote_slot is stored");
    assert_eq!(f.vault(0).reference_price.quote_slot.get(), 1_000_000);

    // Warp forward so quote_slot 0 is well past the old backdate cap.
    f.svm.warp_to_slot(100);
    f.set_reference_price(&signer, 0, valid_price(), 0)
        .expect("stale quote_slot is stored");
    assert_eq!(f.vault(0).reference_price.quote_slot.get(), 0);
}

/// Stamping a frozen vault is a harmless no-op that the write no longer
/// rejects — matching skips frozen vaults regardless, so the guard was
/// dropped to save CU on the common path.
#[test]
fn stamps_a_frozen_vault() {
    let mut f = fixture_with_vault();
    let signer = f.authority.insecure_clone();
    f.freeze_vault(&signer, 0).expect("admin freezes vault");

    f.set_reference_price(&signer, 0, valid_price(), 0)
        .expect("a frozen vault still accepts a reference-price stamp");
    assert_eq!(f.vault(0).reference_price.price.as_u32(), valid_price());
}
