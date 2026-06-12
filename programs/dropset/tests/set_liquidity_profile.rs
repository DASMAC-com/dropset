//! Integration tests for `set_liquidity_profile` — the reference-price
//! precondition, the happy write (profile stored, FLUSH_BIT armed,
//! price untouched), the per-side `size_bps` overflow, and the
//! authority / frozen gates. Built on the shared [`Fixture`].

mod common;

use anchor_lang_v2::bytemuck;
use anchor_v2_testing::Signer;
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
use dropset::{DropsetError, LiquidityProfile, Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Open an admin vault (sector 0) with a reference price already set,
/// so `set_liquidity_profile` clears its `ReferencePriceNotSet` gate.
fn fixture_with_priced_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
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
fn rejects_when_reference_price_not_set() {
    // A freshly-opened vault has no reference price; the profile is pure
    // ppm offsets, so applying it before an anchor price is set would
    // flush to garbage absolute prices. The gate rejects it.
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    let err = f
        .set_liquidity_profile(
            &f.authority.insecure_clone(),
            0,
            simple_profile(5_000, 10_000, u32::MAX),
        )
        .expect_err("set_liquidity_profile must reject before set_reference_price");
    common::assert_program_error(&err, DropsetError::ReferencePriceNotSet);
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
    let signer = f.authority.insecure_clone();
    f.freeze_vault(&signer, 0).expect("admin freezes vault");
    let err = f
        .set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect_err("frozen vault must reject a profile update");
    common::assert_program_error(&err, DropsetError::VaultFrozen);
}
