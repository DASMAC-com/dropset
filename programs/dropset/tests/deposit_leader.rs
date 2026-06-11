//! `deposit_leader` integration tests — the PDA-free leader inventory
//! path. Covers seeding (`total_shares := isqrt(b·q)`,
//! `leader_shares := total_shares`, `hwm := 1.0`), single-leg top-up,
//! and the seeding/single-leg/authority/frozen rejections.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::Q32_32_ONE;
use solana_pubkey::Pubkey;

/// `isqrt` mirror of the on-chain seeding share formula.
fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Bootstrap + open an admin vault (sector 0, leader = authority) with
/// a reference price set. Unseeded.
fn fixture_with_unseeded_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("register_vault");
    let px = dropset::Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f
}

#[test]
fn seeding_sets_shares_to_isqrt_and_hwm_to_one() {
    let mut f = fixture_with_unseeded_vault();
    let (base, quote) = (1_000_000_u64, 1_085_000_u64);
    f.deposit_leader(0, base, quote, base, quote)
        .expect("leader seeds both legs");

    let v = f.vault(0);
    let expected = isqrt(base as u128 * quote as u128) as u64;
    assert_eq!(v.total_shares.get(), expected, "total_shares := isqrt(b·q)");
    assert_eq!(
        v.leader_shares.get(),
        expected,
        "leader_shares := total_shares on seed"
    );
    assert_eq!(v.hwm.get(), Q32_32_ONE, "hwm := 1.0 on seed");
    assert_eq!(v.base_atoms.get(), base);
    assert_eq!(v.quote_atoms.get(), quote);
}

#[test]
fn single_leg_topup_grows_leader_and_total_equally() {
    let mut f = fixture_with_unseeded_vault();
    let (base, quote) = (1_000_000_u64, 1_085_000_u64);
    f.deposit_leader(0, base, quote, base, quote)
        .expect("seed");
    let before = f.vault(0);

    // Top up base-only; the basket ceil pulls a little quote too, but
    // shares grow off the single sized leg.
    f.deposit_leader(0, 100_000, 0, 100_000, 200_000)
        .expect("base-only top-up");

    let after = f.vault(0);
    let d_total = after.total_shares.get() - before.total_shares.get();
    let d_leader = after.leader_shares.get() - before.leader_shares.get();
    assert!(d_total > 0, "top-up minted shares");
    assert_eq!(d_total, d_leader, "leader and total grew by the same delta");
}

#[test]
fn rejects_non_leader_signer() {
    // Open the vault for a *different* leader via the admin override,
    // then have `authority` (not the leader) attempt the deposit.
    let mut f = Fixture::bootstrap();
    let other = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.register_vault(0, f.authority.pubkey(), false, other.pubkey())
        .expect("admin opens vault for `other`");
    let px = dropset::Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, px.as_u32(), 0)
        .expect("set_reference_price");

    let auth = f.authority.insecure_clone();
    let err = f
        .deposit_leader_as(&auth, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect_err("non-leader signer must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn rejects_seeding_with_one_leg_zero() {
    let mut f = fixture_with_unseeded_vault();
    let err = f
        .deposit_leader(0, 1_000_000, 0, 1_000_000, 0)
        .expect_err("seeding with a zero leg must reject");
    common::assert_program_error(&err, dropset::DropsetError::SeedingRequiresBothLegs);
}

#[test]
fn rejects_non_seeding_with_both_legs() {
    let mut f = fixture_with_unseeded_vault();
    f.deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");
    // Post-seed, supplying both legs is ambiguous — exactly one leg
    // must be sized on a top-up.
    let err = f
        .deposit_leader(0, 100_000, 100_000, 200_000, 200_000)
        .expect_err("non-seeding deposit with both legs must reject");
    common::assert_program_error(&err, dropset::DropsetError::SingleLegRequired);
}

#[test]
fn rejects_frozen_vault() {
    let mut f = fixture_with_unseeded_vault();
    f.poke_frozen(0, true);
    let err = f
        .deposit_leader(0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect_err("frozen vault must reject leader deposit");
    common::assert_program_error(&err, dropset::DropsetError::VaultFrozen);
}
