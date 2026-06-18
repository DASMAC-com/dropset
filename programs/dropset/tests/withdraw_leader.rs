//! `withdraw_leader` integration tests — the PDA-free leader exit
//! path. Covers the full drain, the `min_leader_share` floor on a
//! partial exit (with an outside depositor present so the ratio can
//! actually fall), and the authority / share-balance rejections.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use solana_pubkey::Pubkey;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

#[test]
fn full_exit_drains_inventory_to_leader() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.pubkey();
    let leader_shares = f.vault(0).leader_shares.get();
    assert!(leader_shares > 0);

    f.withdraw_leader(0, leader_shares, 0, 0)
        .expect("leader fully exits");

    let v = f.vault(0);
    assert_eq!(v.leader_shares.get(), 0, "leader stake burned");
    assert_eq!(v.total_shares.get(), 0, "no shares remain");
    assert_eq!(v.base_atoms.get(), 0, "base inventory drained");
    assert_eq!(v.quote_atoms.get(), 0, "quote inventory drained");
    assert!(
        f.token_balance(&f.base_ata(&leader)) > 0,
        "leader received base"
    );
    assert!(
        f.token_balance(&f.quote_ata(&leader)) > 0,
        "leader received quote"
    );
    // Treasury invariant: emptied alongside the vault.
    assert_eq!(f.token_balance(&f.base_treasury), 0);
    assert_eq!(f.token_balance(&f.quote_treasury), 0);
}

#[test]
fn full_exit_reclaims_sector() {
    // A leader who is the sole shareholder and burns their entire stake
    // drives `total_shares` to 0, which must return the sector to the
    // free DLL — mirroring `force_withdraw_leader`. Before the ENG-462
    // fix the signed leader path left the drained sector threaded on the
    // active list with a non-default `leader`, leaking the slab slot and
    // never decrementing the `active_count` it held.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    assert_eq!(f.market_header().active_count.get(), 1, "one active vault");
    let leader_shares = f.vault(0).leader_shares.get();
    assert!(leader_shares > 0);

    f.withdraw_leader(0, leader_shares, 0, 0)
        .expect("leader fully exits");

    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault fully drained");
    // Sector reclaimed to the free DLL: zeroed leader, off the active
    // list, free head pointing at it.
    assert_eq!(
        v.leader,
        Pubkey::default().to_bytes().into(),
        "reclaim zeroes the leader marker"
    );
    let h = f.market_header();
    assert_eq!(
        h.active_count.get(),
        0,
        "active count decremented on reclaim"
    );
    assert_eq!(h.head.get(), dropset::NULL_SECTOR, "active list now empty");
    assert_eq!(
        h.free_head.get(),
        0,
        "sector 0 reclaimed onto the free list"
    );
}

#[test]
fn tombstoned_full_exit_reclaims_sector() {
    // Tombstone variant of `full_exit_reclaims_sector`: the leader is the
    // sole shareholder, tombstones the vault (active_count → 0, sector on
    // the tombstone DLL), then fully exits with the floor bypassed. The
    // final burn zeroes `total_shares`, which must reclaim the sector off
    // the *tombstone* list — the `DllList::Tombstone` branch of
    // `reclaim_sector`, where `active_count` is not decremented again.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.insecure_clone();

    f.close_vault(&leader, 0)
        .expect("leader tombstones the vault");
    assert_eq!(
        f.market_header().active_count.get(),
        0,
        "tombstoning drops the active count"
    );

    let leader_shares = f.vault(0).leader_shares.get();
    f.withdraw_leader(0, leader_shares, 0, 0)
        .expect("leader fully exits the tombstoned vault");

    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault fully drained");
    assert_eq!(
        v.leader,
        Pubkey::default().to_bytes().into(),
        "reclaim zeroes the leader marker"
    );
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 0, "active count stays zero");
    assert_eq!(
        h.tombstone_head.get(),
        dropset::NULL_SECTOR,
        "tombstone list now empty"
    );
    assert_eq!(
        h.free_head.get(),
        0,
        "sector 0 reclaimed onto the free list"
    );
}

#[test]
fn rejects_non_leader_signer() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .withdraw_leader_as(&stranger, 0, 1, 0, 0)
        .expect_err("non-leader signer must be rejected");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn rejects_shares_over_leader_balance() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let leader_shares = f.vault(0).leader_shares.get();
    let err = f
        .withdraw_leader(0, leader_shares + 1, 0, 0)
        .expect_err("shares_in over leader balance must reject");
    common::assert_program_error(&err, dropset::DropsetError::InsufficientShares);
}

#[test]
fn rejects_zero_shares() {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let err = f
        .withdraw_leader(0, 0, 0, 0)
        .expect_err("zero shares must reject");
    common::assert_program_error(&err, dropset::DropsetError::InsufficientShares);
}

#[test]
fn partial_exit_violating_floor_rejects() {
    // Add an outside depositor so `leader_shares < total_shares` and a
    // leader withdrawal can actually drop the ratio. Then pin
    // `min_leader_share` at the post-deposit ratio and withdraw a big
    // chunk — the post-burn ratio falls below the floor.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();

    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 100_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    let v = f.vault(0);
    let (l, t) = (v.leader_shares.get() as u128, v.total_shares.get() as u128);
    assert!(l < t, "leader is no longer the sole shareholder");
    // Pin the floor at the current ratio (set after the deposit so the
    // deposit's own floor check isn't affected).
    let ratio_ppm = (l * 1_000_000 / t) as u32;
    f.set_min_leader_share(&admin, 0, ratio_ppm).unwrap();

    // Withdraw 90% of the leader's stake — the post-burn ratio drops
    // well below `ratio_ppm`.
    let big = (l * 9 / 10) as u64;
    let err = f
        .withdraw_leader(0, big, 0, 0)
        .expect_err("withdrawal below the leader-share floor must reject");
    common::assert_program_error(&err, dropset::DropsetError::MinLeaderShareViolated);
}

#[test]
fn tombstoned_vault_bypasses_floor() {
    // Same setup as `partial_exit_violating_floor_rejects` — an outside
    // depositor and a floor pinned at the post-deposit ratio so a large
    // leader withdrawal would otherwise trip `MinLeaderShareViolated`.
    // Tombstoning the vault (the leader's orderly wind-down) must lift
    // the skin-in-the-game floor so the leader can exit their stake,
    // matching the spec's frozen/tombstoned bypass. Tombstone status is
    // tracked independently of `frozen`, so the floor guard has to read
    // it too.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();

    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 100_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    let v = f.vault(0);
    let (l, t) = (v.leader_shares.get() as u128, v.total_shares.get() as u128);
    assert!(l < t, "leader is no longer the sole shareholder");
    let ratio_ppm = (l * 1_000_000 / t) as u32;
    f.set_min_leader_share(&admin, 0, ratio_ppm).unwrap();

    // Move the vault to the tombstone DLL — the leader's wind-down path.
    f.close_vault(&leader, 0)
        .expect("leader tombstones the vault");

    // The same 90% withdrawal that rejects on an active vault now
    // succeeds: tombstoning lifts the floor.
    let big = (l * 9 / 10) as u64;
    f.withdraw_leader(0, big, 0, 0)
        .expect("tombstoned vault must bypass the leader-share floor");

    assert_eq!(
        f.vault(0).leader_shares.get(),
        (l as u64) - big,
        "leader stake burned despite the floor"
    );
}
