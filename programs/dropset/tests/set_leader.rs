//! `set_leader` integration tests — leader-only custody rotation,
//! co-signed by the incoming key. Covers the happy rotation and its event,
//! the functional hand-off (the new leader gains the leader levers and the
//! old one loses them), what the rotation leaves alone (`seq`, the quoting
//! delegate), rotation on tombstoned vaults (refused on frozen ones), and
//! each gate.
//!
//! The zero-address gate has no test: no key signs for the zero address,
//! so the co-sign makes it unreachable from a real transaction. The
//! handler keeps it as a local statement of the invariant.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::{events, fixture::Fixture};
use dropset::DropsetError;
use solana_pubkey::Pubkey;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

fn new_key(f: &mut Fixture) -> Keypair {
    f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS)
}

#[test]
fn leader_rotates_and_emits_event() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    // A delegate distinct from both leaders, so the event's
    // `quote_authority` slot can't pass by coinciding with either.
    let delegate = Pubkey::new_unique();
    f.set_quote_authority(&leader, 0, delegate)
        .expect("delegate quoting");
    let seq_before = f.vault(0).seq.get();

    let meta = f
        .set_leader_meta(&leader, &next, 0)
        .expect("leader may rotate to a co-signing key");

    let vault = f.vault(0);
    assert_eq!(vault.leader, next.pubkey().to_bytes().into());
    // Same vault under a new key, not a new vault.
    assert_eq!(vault.seq.get(), seq_before);

    let ev = events::set_leader(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.vault_seq, seq_before);
    assert_eq!(ev.old_leader, leader.pubkey().to_bytes());
    assert_eq!(ev.new_leader, next.pubkey().to_bytes());
    assert_eq!(ev.quote_authority, delegate.to_bytes());
}

#[test]
fn quote_authority_does_not_follow() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    let delegate = Pubkey::new_unique();
    f.set_quote_authority(&leader, 0, delegate)
        .expect("delegate quoting");

    f.set_leader(&leader, &next, 0).expect("rotate");
    // Ratified: no auto-follow. The delegate keeps quoting until the new
    // leader rotates it.
    assert_eq!(f.vault(0).quote_authority, delegate.to_bytes().into());
}

#[test]
fn rotation_hands_off_the_leader_levers() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    f.set_leader(&leader, &next, 0).expect("rotate");

    // The new leader now owns the leader-only levers...
    f.set_quote_authority(&next, 0, next.pubkey())
        .expect("rotated-in leader may rotate the quoting delegate");
    // ...and the old one has lost them.
    let err = f
        .set_quote_authority(&leader, 0, leader.pubkey())
        .expect_err("the superseded leader must lose the leader levers");
    common::assert_program_error(&err, DropsetError::Unauthorized);
}

#[test]
fn rejects_rotation_on_a_frozen_vault() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    f.freeze_vault(&leader, 0).expect("admin freezes vault");

    // Rotating onto a second key would free the frozen leader to open a
    // fresh vault on the market.
    let next = new_key(&mut f);
    let err = f
        .set_leader(&leader, &next, 0)
        .expect_err("a frozen vault's leader is pinned");
    common::assert_program_error(&err, DropsetError::VaultFrozen);
    assert_eq!(f.vault(0).leader, leader.pubkey().to_bytes().into());
}

#[test]
fn leader_may_rotate_on_a_tombstoned_vault() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    f.close_vault(&leader, 0).expect("leader closes vault");
    assert!(f.vault(0).tombstoned.get());

    let next = new_key(&mut f);
    f.set_leader(&leader, &next, 0)
        .expect("a winding-down vault's custody may still move");
    assert_eq!(f.vault(0).leader, next.pubkey().to_bytes().into());
}

#[test]
fn rejects_non_leader() {
    let mut f = Fixture::with_vault();
    let stranger = new_key(&mut f);
    let next = new_key(&mut f);
    let err = f
        .set_leader(&stranger, &next, 0)
        .expect_err("only the leader may rotate");
    common::assert_program_error(&err, DropsetError::Unauthorized);
    assert_eq!(f.vault(0).leader, f.authority.pubkey().to_bytes().into());
}

#[test]
fn rejects_same_leader() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let err = f
        .set_leader(&leader, &leader, 0)
        .expect_err("a no-op rotation must reject");
    common::assert_program_error(&err, DropsetError::SameLeader);
}

#[test]
fn rejects_new_leader_of_a_live_vault() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    // Admin override seats `next` as the leader of sector 1.
    f.create_vault(0, next.pubkey(), false, next.pubkey())
        .expect("seat next on a second vault");

    let err = f
        .set_leader(&leader, &next, 0)
        .expect_err("one live vault per leader per market");
    common::assert_program_error(&err, DropsetError::LeaderAlreadyLeadsVault);
}

#[test]
fn rejects_new_leader_of_a_frozen_vault() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    f.create_vault(0, next.pubkey(), false, next.pubkey())
        .expect("seat next on a second vault");
    f.freeze_vault(&leader, 1).expect("admin freezes sector 1");

    // A frozen vault counts, exactly as at `create_vault`.
    let err = f
        .set_leader(&leader, &next, 0)
        .expect_err("leading a frozen vault blocks a rotation");
    common::assert_program_error(&err, DropsetError::LeaderAlreadyLeadsVault);
}

#[test]
fn rejects_new_leader_holding_a_depositor_position() {
    let mut f = Fixture::seeded_open(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.insecure_clone();
    let alice = f.outside_deposit();

    let err = f
        .set_leader(&leader, &alice, 0)
        .expect_err("a vault's leader never holds a depositor position on it");
    common::assert_program_error(&err, DropsetError::NewLeaderIsDepositor);
}

#[test]
fn accepts_new_leader_after_their_position_closes() {
    let mut f = Fixture::seeded_open(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.insecure_clone();
    let alice = f.outside_deposit();
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    f.withdraw(&alice, 0, shares, 0, 0)
        .expect("full withdraw closes the PDA");

    f.set_leader(&leader, &alice, 0)
        .expect("a closed position no longer blocks the rotation");
}

#[test]
fn dust_on_the_depositor_address_does_not_block() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    // Anyone can fund the PDA address; that must not read as a position.
    let (pda, _) = common::fixture::vault_depositor_pda(&f.market, 0, &next.pubkey());
    f.svm
        .airdrop(&pda, 1_000_000)
        .expect("dust the PDA address");

    f.set_leader(&leader, &next, 0)
        .expect("a funded but unowned address is not a depositor position");
}

#[test]
fn rejects_rotation_the_new_leader_did_not_sign() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = Pubkey::new_unique();
    let (pda, _) = common::fixture::vault_depositor_pda(&f.market, 0, &next);
    // The co-signature is the liveness proof: without it, custody could be
    // pushed onto a key nobody holds.
    let ix = f.set_leader_ix(&leader.pubkey(), &next, &pda, false, 0);
    f.send_ix(&leader, ix)
        .expect_err("the incoming leader must co-sign");
    assert_eq!(f.vault(0).leader, leader.pubkey().to_bytes().into());
}

#[test]
fn rejects_a_substituted_depositor_account() {
    let mut f = Fixture::seeded_open(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.insecure_clone();
    let alice = f.outside_deposit();

    // Alice holds a position, and tries to dodge the depositor gate by
    // passing someone else's (empty) PDA in place of her own. The seeds
    // constraint must reject it before the handler's ownership test runs.
    let (other_pda, _) = common::fixture::vault_depositor_pda(&f.market, 0, &Pubkey::new_unique());
    let ix = f.set_leader_ix(&leader.pubkey(), &alice.pubkey(), &other_pda, true, 0);
    common::send_ixn_signed_meta(&mut f.svm, &[&leader, &alice], ix)
        .expect_err("the depositor PDA must be the new leader's own");
    assert_eq!(f.vault(0).leader, leader.pubkey().to_bytes().into());
}

#[test]
fn rejects_invalid_idx() {
    let mut f = Fixture::with_vault();
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    let err = f
        .set_leader(&leader, &next, 99)
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, DropsetError::InvalidSectorIndex);
}

#[test]
fn rejects_empty_sector() {
    let mut f = Fixture::with_vault();
    // Vacate the sector by zeroing its leader — the free-list marker. The
    // VaultEmpty guard must fire before the leader-authorization check.
    f.poke_leader_empty(0);
    let leader = f.authority.insecure_clone();
    let next = new_key(&mut f);
    let err = f
        .set_leader(&leader, &next, 0)
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, DropsetError::VaultEmpty);
}
