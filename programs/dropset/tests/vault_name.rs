//! Vault-name integration tests — the name stamped by `create_vault`, the
//! leader's `set_vault_name`, the admin's `override_vault_name` and its
//! lock, and what `freeze_vault`, `close_vault`, `set_leader` and sector
//! reclaim do to both. Covers validity, per-market case-insensitive
//! uniqueness (an unset name never collides), and every emitted name field.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::{events, fixture::Fixture, SIGNER_FUNDING_LAMPORTS};
use dropset::{decode_vault_name, DropsetError};
use solana_pubkey::Pubkey;

/// Bootstrap + one admin vault (sector 0) led by `authority`, named `name`.
fn fixture_named(name: &str) -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault_named_meta(0, f.authority.pubkey(), false, Pubkey::default(), name)
        .expect("create_vault");
    f
}

/// Seat a second vault (sector 1) under a fresh funded leader, named `name`.
fn second_vault(f: &mut Fixture, name: &str) -> Result<Keypair, String> {
    let leader = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    f.create_vault_named_meta(0, leader.pubkey(), false, leader.pubkey(), name)
        .map(|_| leader)
}

fn name_of(f: &Fixture, idx: u32) -> String {
    decode_vault_name(&f.vault(idx).name)
}

#[test]
fn create_stamps_name_and_emits_it() {
    let mut f = Fixture::bootstrap();
    let meta = f
        .create_vault_named_meta(
            0,
            f.authority.pubkey(),
            false,
            Pubkey::default(),
            "AUDD Desk",
        )
        .expect("create_vault");
    assert_eq!(name_of(&f, 0), "AUDD Desk");
    assert!(!f.vault(0).name_locked.get());
    assert_eq!(events::create_vault(&meta).name, "AUDD Desk");
}

#[test]
fn create_without_a_name_leaves_it_unset() {
    let mut f = Fixture::bootstrap();
    let meta = f
        .create_vault_meta(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    assert_eq!(f.vault(0).name, [0; 32]);
    assert_eq!(events::create_vault(&meta).name, "");
}

#[test]
fn create_rejects_invalid_names() {
    let mut f = Fixture::bootstrap();
    let too_long = "x".repeat(33);
    for bad in [too_long.as_str(), "caf\u{e9}", "tab\there", "nul\0"] {
        let err = f
            .create_vault_named_meta(0, f.authority.pubkey(), false, Pubkey::default(), bad)
            .expect_err("invalid name must reject");
        common::assert_program_error(&err, DropsetError::InvalidVaultName);
    }
    // The 32-byte edge is accepted.
    f.create_vault_named_meta(
        0,
        f.authority.pubkey(),
        false,
        Pubkey::default(),
        &"x".repeat(32),
    )
    .expect("a full-width name is valid");
}

#[test]
fn create_rejects_a_case_folded_duplicate() {
    let mut f = fixture_named("Alpha");
    let err = second_vault(&mut f, "aLPHA").expect_err("duplicate name must reject");
    common::assert_program_error(&err, DropsetError::VaultNameTaken);
    assert_eq!(f.market_header().active_count.get(), 1);
    second_vault(&mut f, "Alpha 2").expect("a distinct name is free");
}

#[test]
fn unset_names_never_collide() {
    let mut f = fixture_named("");
    second_vault(&mut f, "").expect("two unset names coexist");
    assert_eq!(f.market_header().active_count.get(), 2);
}

#[test]
fn leader_renames_and_emits_both_names() {
    let mut f = fixture_named("Alpha");
    let leader = f.authority.insecure_clone();
    let seq = f.vault(0).seq.get();
    let meta = f
        .set_vault_name_meta(&leader, 0, "Beta")
        .expect("leader may rename");
    assert_eq!(name_of(&f, 0), "Beta");

    let ev = events::set_vault_name(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.vault_seq, seq);
    assert_eq!(ev.leader, leader.pubkey().to_bytes());
    assert_eq!(ev.old_name, "Alpha");
    assert_eq!(ev.new_name, "Beta");
}

#[test]
fn leader_may_re_case_and_blank_its_own_name() {
    let mut f = fixture_named("Alpha");
    let leader = f.authority.insecure_clone();
    f.set_vault_name_meta(&leader, 0, "ALPHA")
        .expect("a vault never collides with itself");
    assert_eq!(name_of(&f, 0), "ALPHA");
    f.set_vault_name_meta(&leader, 0, "")
        .expect("blanking is a rename");
    assert_eq!(f.vault(0).name, [0; 32]);
}

#[test]
fn rename_rejects_a_taken_name_and_an_invalid_one() {
    let mut f = fixture_named("Alpha");
    let other = second_vault(&mut f, "Beta").expect("second vault");
    let err = f
        .set_vault_name_meta(&other, 1, "alpha")
        .expect_err("taken name must reject");
    common::assert_program_error(&err, DropsetError::VaultNameTaken);
    let err = f
        .set_vault_name_meta(&other, 1, &"y".repeat(33))
        .expect_err("invalid name must reject");
    common::assert_program_error(&err, DropsetError::InvalidVaultName);
    assert_eq!(name_of(&f, 1), "Beta");
}

#[test]
fn rename_rejects_non_leader() {
    let mut f = fixture_named("Alpha");
    let stranger = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_vault_name_meta(&stranger, 0, "Mine")
        .expect_err("non-leader must not rename");
    common::assert_program_error(&err, DropsetError::Unauthorized);
}

#[test]
fn rename_rejects_an_empty_sector() {
    let mut f = fixture_named("Alpha");
    f.poke_leader_empty(0);
    let leader = f.authority.insecure_clone();
    let err = f
        .set_vault_name_meta(&leader, 0, "Beta")
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, DropsetError::VaultEmpty);
}

#[test]
fn override_replaces_locks_and_emits() {
    let mut f = fixture_named("Alpha");
    let admin = f.authority.insecure_clone();
    let seq = f.vault(0).seq.get();
    let meta = f
        .override_vault_name_meta(&admin, 0, "Renamed", true)
        .expect("admin override");
    assert_eq!(name_of(&f, 0), "Renamed");
    assert!(f.vault(0).name_locked.get());

    let ev = events::override_vault_name(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.vault_seq, seq);
    assert_eq!(ev.admin, admin.pubkey().to_bytes());
    assert!(ev.lock);
    assert_eq!(ev.old_name, "Alpha");
    assert_eq!(ev.new_name, "Renamed");
}

#[test]
fn lock_refuses_the_leader_until_an_unlock() {
    let mut f = fixture_named("Alpha");
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.override_vault_name_meta(&admin, 0, "", true)
        .expect("admin blanks and locks");
    assert_eq!(f.vault(0).name, [0; 32]);

    let err = f
        .set_vault_name_meta(&leader, 0, "Back")
        .expect_err("a locked name refuses the leader");
    common::assert_program_error(&err, DropsetError::VaultNameLocked);

    let ev = events::override_vault_name(
        &f.override_vault_name_meta(&admin, 0, "", false)
            .expect("admin unlocks"),
    );
    assert!(!ev.lock);
    assert!(!f.vault(0).name_locked.get());
    // A distinct name, so the txn differs from the refused one above.
    f.set_vault_name_meta(&leader, 0, "Back again")
        .expect("unlocked, the leader renames again");
    assert_eq!(name_of(&f, 0), "Back again");
}

#[test]
fn override_obeys_uniqueness_and_admin_gate() {
    let mut f = fixture_named("Alpha");
    second_vault(&mut f, "Beta").expect("second vault");
    let admin = f.authority.insecure_clone();
    let err = f
        .override_vault_name_meta(&admin, 1, "ALPHA", false)
        .expect_err("override cannot take a live name");
    common::assert_program_error(&err, DropsetError::VaultNameTaken);
    let err = f
        .override_vault_name_meta(&admin, 1, "caf\u{e9}", false)
        .expect_err("override obeys the validity rules too");
    common::assert_program_error(&err, DropsetError::InvalidVaultName);

    let stranger = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    let err = f
        .override_vault_name_meta(&stranger, 1, "Gamma", true)
        .expect_err("non-admin must not override");
    common::assert_program_error(&err, DropsetError::Unauthorized);
    assert_eq!(name_of(&f, 1), "Beta");
}

#[test]
fn freeze_blanks_locks_emits_and_frees_the_name() {
    let mut f = fixture_named("Alpha");
    let admin = f.authority.insecure_clone();
    let meta = f.freeze_vault_meta(&admin, 0).expect("freeze");
    assert_eq!(events::freeze_vault(&meta).name, "Alpha");
    assert_eq!(f.vault(0).name, [0; 32]);
    assert!(f.vault(0).name_locked.get());

    let err = f
        .set_vault_name_meta(&admin, 0, "Again")
        .expect_err("a frozen vault cannot be renamed by its leader");
    common::assert_program_error(&err, DropsetError::VaultFrozen);
    // The released name is free for another leader. The frozen leader
    // itself may not re-enter while its vault is unreclaimed.
    second_vault(&mut f, "alpha").expect("freeze releases the name");
    let err = f
        .create_vault_named_meta(0, f.authority.pubkey(), false, Pubkey::default(), "Other")
        .expect_err("a frozen vault still counts against its leader");
    common::assert_program_error(&err, DropsetError::LeaderAlreadyLeadsVault);
}

#[test]
fn override_reaches_a_frozen_vault() {
    // The admin path is not refused where the leader's is: it is the only
    // way to name or unlock a frozen vault.
    let mut f = fixture_named("Alpha");
    let admin = f.authority.insecure_clone();
    f.freeze_vault_meta(&admin, 0).expect("freeze");
    f.override_vault_name_meta(&admin, 0, "Frozen Desk", false)
        .expect("admin overrides a frozen vault");
    assert_eq!(name_of(&f, 0), "Frozen Desk");
    assert!(!f.vault(0).name_locked.get());
}

#[test]
fn leader_cannot_rename_a_tombstone_to_shed_a_lock() {
    // Close blanks the name and clears the lock; refusing the tombstone
    // rename is what keeps that release final. Seeded, since closing a
    // never-seeded vault reclaims it rather than tombstoning it.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    let leader = f.authority.insecure_clone();
    f.override_vault_name_meta(&leader, 0, "Alpha", true)
        .expect("admin locks the name");
    f.close_vault_meta(&leader, 0).expect("close");
    let err = f
        .set_vault_name_meta(&leader, 0, "Alpha")
        .expect_err("a tombstoned vault cannot be renamed by its leader");
    common::assert_program_error(&err, DropsetError::VaultTombstoned);
    assert_eq!(f.vault(0).name, [0; 32]);
}

#[test]
fn close_blanks_unlocks_emits_and_frees_the_name() {
    let mut f = fixture_named("Alpha");
    let leader = f.authority.insecure_clone();
    f.override_vault_name_meta(&leader, 0, "Alpha", true)
        .expect("admin locks the name");
    let meta = f.close_vault_meta(&leader, 0).expect("close");
    assert_eq!(events::close_vault(&meta).name, "Alpha");
    assert_eq!(f.vault(0).name, [0; 32]);
    assert!(!f.vault(0).name_locked.get());

    second_vault(&mut f, "ALPHA").expect("close releases the name");
}

#[test]
fn rotation_keeps_name_and_lock() {
    let mut f = fixture_named("Alpha");
    let leader = f.authority.insecure_clone();
    f.override_vault_name_meta(&leader, 0, "Alpha", true)
        .expect("admin locks the name");
    let new_leader = f.funded_keypair(SIGNER_FUNDING_LAMPORTS);
    f.set_leader(&leader, &new_leader, 0).expect("rotate");
    assert_eq!(name_of(&f, 0), "Alpha");
    assert!(f.vault(0).name_locked.get());
}

#[test]
fn reclaim_and_reuse_reset_name_and_lock() {
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    let leader = f.authority.insecure_clone();
    f.close_vault(&leader, 0).expect("tombstone");
    // A tombstone still counts: an admin can name and lock it, and the
    // name is then taken.
    f.override_vault_name_meta(&leader, 0, "Ghost", true)
        .expect("admin names the tombstone");
    let err = second_vault(&mut f, "ghost").expect_err("a tombstone's name is taken");
    common::assert_program_error(&err, DropsetError::VaultNameTaken);

    let shares = f.vault(0).leader_shares.get();
    f.withdraw_leader(0, shares, 0, 0)
        .expect("full exit reclaims");
    assert_eq!(f.market_header().free_head.get(), 0, "sector 0 is free");

    // The reclaimed sector no longer holds the name; reuse zeroes the lock.
    f.create_vault_named_meta(1, f.authority.pubkey(), false, Pubkey::default(), "Ghost")
        .expect("reclaim releases the name");
    assert_eq!(f.market_header().head.get(), 0, "free sector 0 reused");
    assert_eq!(name_of(&f, 0), "Ghost");
    assert!(!f.vault(0).name_locked.get());
}
