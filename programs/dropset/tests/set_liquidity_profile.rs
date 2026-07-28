//! Integration tests for `set_liquidity_profile` — the happy write
//! (profile stored, FLUSH_BIT armed, price untouched), the single
//! write-time domain gate (`quote_authority`), the sector-bounds gate, and
//! the three conditions the write deliberately *accepts* because matching
//! enforces them instead: an over-cap `Σ size_bps`, a ladder armed before a
//! reference price, and a frozen vault. The over-cap case carries the
//! end-to-end pairing too — write, then take, and watch the book drop the
//! offending side. Built on the shared [`Fixture`].

mod common;

use anchor_lang_v2::bytemuck;
use anchor_v2_testing::Signer;
use common::fixture::{simple_profile, Fixture, PROFILE_BYTES};
use dropset::{DropsetError, LiquidityProfile, Price, FLUSH_BIT};
use solana_pubkey::Pubkey;

/// Open an admin vault (sector 0) with a reference price already set — the
/// normal lifecycle order (`CreateVault` → `SetReferencePrice` →
/// `SetLiquidityProfile`), which every test but
/// [`stores_a_profile_before_any_reference_price`] follows.
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

/// The write no longer gates on a set reference price. A ladder armed ahead
/// of its anchor is inert rather than dangerous: matching gates the whole
/// vault on `has_valid_reference_price()` before the flush, so the profile
/// never materializes to garbage absolute prices, and `FLUSH_BIT` stays
/// armed until a real price lands.
#[test]
fn stores_a_profile_before_any_reference_price() {
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

/// `Σ size_bps > BPS` is enforced at match time, not write time: the flush
/// zeroes the offending side out of the book (see the
/// `materialize_remaining` coverage in `state/market/accrual.rs`, and
/// `over_cap_ladder_is_dropped_from_the_book` below for the end-to-end
/// path) instead of hard-rejecting the write, so an over-cap ladder stores
/// verbatim and the leader self-heals with their next valid profile.
#[test]
fn stores_an_over_cap_bid_side_raw() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    let bytes = oversized_profile(true);

    f.set_liquidity_profile(&signer, 0, bytes)
        .expect("an over-cap bid side is stored, not rejected");

    let v = f.vault(0);
    assert_eq!(v.profile.bids[0].size_bps.get(), 6_000);
    assert_eq!(v.profile.bids[1].size_bps.get(), 5_000);
}

/// Mirror of [`stores_an_over_cap_bid_side_raw`] on the ask side.
#[test]
fn stores_an_over_cap_ask_side_raw() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();

    f.set_liquidity_profile(&signer, 0, oversized_profile(false))
        .expect("an over-cap ask side is stored, not rejected");

    let v = f.vault(0);
    assert_eq!(v.profile.asks[0].size_bps.get(), 6_000);
    assert_eq!(v.profile.asks[1].size_bps.get(), 5_000);
}

/// Neither quote-write path re-reads `frozen`: the freeze is enforced at
/// match time, where `swap` skips frozen vaults entirely, so re-quoting one
/// is a no-op the ASM fast path need not spend CU rejecting. Mirrors the
/// sibling `set_reference_price` test `stamps_a_frozen_vault`.
#[test]
fn reshapes_a_frozen_vault() {
    let mut f = fixture_with_priced_vault();
    let signer = f.authority.insecure_clone();
    f.freeze_vault(&signer, 0).expect("admin freezes vault");

    f.set_liquidity_profile(&signer, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("a frozen vault accepts an inert profile update");

    let v = f.vault(0);
    assert!(v.frozen.get(), "still frozen");
    assert_eq!(v.profile.asks[0].size_bps.get(), 10_000, "profile stored");
}

/// The end-to-end pairing the removed write-time reject leaves behind:
/// write an over-cap ladder **through the instruction**, then take against
/// it. The offending side must be dropped from the book (no fill) while the
/// stored bytes survive for the leader to self-heal — the state that was
/// previously reachable only by poking corrupt bytes into the account (see
/// `swap.rs`'s `oversize_ask_side_skips_that_vault_but_not_the_swap`).
#[test]
fn over_cap_ladder_is_dropped_from_the_book() {
    // Ask side sums to 11_000 bps > BPS; bids are left empty.
    let mut f = Fixture::seeded_with(1_000_000, 1_085_000, oversized_profile(false));
    let taker = f.funded_depositor(0, 200_000);
    let base_before = f.vault(0).base_atoms.get();

    // `min_out: 0` so the no-fill take is an empty result rather than a
    // min-out rejection — the point is that matching skips the side, not
    // that it aborts the taker.
    f.swap(&taker, 0, 50_000, Price::INFINITY.as_u32(), 0)
        .expect("an over-cap ask side must not abort the taker's swap");

    assert_eq!(
        f.vault(0).base_atoms.get(),
        base_before,
        "the over-cap ask side took no fill — not even rounding dust"
    );
    // The flush zeroed `remaining`, never the stored profile.
    let v = f.vault(0);
    assert_eq!(v.profile.asks[0].size_bps.get(), 6_000, "profile intact");
    assert_eq!(v.profile.asks[1].size_bps.get(), 5_000);
    assert_eq!(v.remaining.asks[0].size.get(), 0, "side zeroed at flush");
    assert_eq!(v.remaining.asks[1].size.get(), 0);
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
