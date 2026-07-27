//! Integration tests for `set_liquidity_profile` — the happy write
//! (profile stored, FLUSH_BIT armed, price untouched), the single
//! write-time domain gate (`quote_authority`), the sector-bounds gate, and
//! the three conditions the write deliberately *accepts* because matching
//! enforces them instead: an over-cap `Σ size_bps`, a ladder armed before a
//! reference price, and a frozen vault. Built on the shared [`Fixture`].

mod common;

use anchor_lang_v2::bytemuck;
use anchor_v2_testing::Signer;
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
use dropset::{DropsetError, LiquidityProfile, Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Open an admin vault (sector 0) with a reference price already set — the
/// normal lifecycle order (`CreateVault` → `SetReferencePrice` →
/// `SetLiquidityProfile`), which every test but
/// [`accepts_write_before_reference_price_is_set`] follows.
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
fn accepts_write_before_reference_price_is_set() {
    // The write no longer gates on a set reference price. A ladder armed
    // ahead of its anchor is inert rather than dangerous: matching gates the
    // whole vault on `has_valid_reference_price()` before the flush, so the
    // profile never materializes to garbage absolute prices, and FLUSH_BIT
    // stays armed until a real price lands.
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    let signer = f.authority.insecure_clone();

    f.set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("a price-less vault accepts a profile");

    let v = f.vault(0);
    assert_eq!(v.profile.asks[0].size_bps.get(), 10_000, "profile stored");
    assert!(
        v.reference_price.stamp.get() & FLUSH_BIT != 0,
        "flush armed"
    );
    assert!(
        !v.has_valid_reference_price(),
        "still parked out of the book until a price is stamped"
    );
}

#[test]
fn accepts_bid_size_overflow() {
    // Σ size_bps > BPS is enforced at match time, not write time: the flush
    // zeroes the offending side out of the book (see the
    // `materialize_remaining` coverage in `state/market/accrual.rs`) instead
    // of hard-rejecting the write, so an over-cap ladder stores verbatim and
    // the leader self-heals with their next valid profile.
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    let bytes = oversized_profile(true);

    f.set_liquidity_profile(&signer, 0, bytes)
        .expect("an over-cap bid side is stored, not rejected");

    let v = f.vault(0);
    assert_eq!(v.profile.bids[0].size_bps.get(), 6_000);
    assert_eq!(v.profile.bids[1].size_bps.get(), 5_000);
}

#[test]
fn accepts_ask_size_overflow() {
    // Mirror of `accepts_bid_size_overflow` on the ask side.
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();

    f.set_liquidity_profile(&signer, 0, oversized_profile(false))
        .expect("an over-cap ask side is stored, not rejected");

    let v = f.vault(0);
    assert_eq!(v.profile.asks[0].size_bps.get(), 6_000);
    assert_eq!(v.profile.asks[1].size_bps.get(), 5_000);
}

#[test]
fn accepts_write_to_frozen_vault() {
    // Neither quote-write path re-reads `frozen`: the freeze is enforced at
    // match time, where `swap` skips frozen vaults entirely, so re-quoting
    // one is a no-op the ASM fast path need not spend CU rejecting.
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    f.freeze_vault(&signer, 0).expect("admin freezes vault");

    f.set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("a frozen vault accepts an inert profile update");

    let v = f.vault(0);
    assert!(v.frozen.get(), "still frozen");
    assert_eq!(v.profile.asks[0].size_bps.get(), 10_000, "profile stored");
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
fn rejects_out_of_range_sector() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    let err = f
        .set_liquidity_profile(&signer, 99, simple_profile(5_000, 10_000, u32::MAX))
        .expect_err("vault_idx past the slab length must reject");
    common::assert_program_error(&err, DropsetError::InvalidSectorIndex);
}
