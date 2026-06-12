//! Integration tests for the two-key outside-deposit gate setters
//! (`set_allow_outside_depositors`, `set_outside_deposits_approved`).

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use solana_pubkey::Pubkey;

/// Open an admin vault on a fresh market; leader + quote authority are
/// both `f.authority`. Both outside flags start `false`.
fn fixture_with_vault() -> Fixture {
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
        .expect("create_vault");
    f
}

// ── set_allow_outside_depositors (leader-only) ───────────────────────

#[test]
fn allow_outside_depositors_leader_flips_flag() {
    let mut f = fixture_with_vault();
    assert!(!f.vault(0).allow_outside_depositors.get());

    let leader = f.authority.insecure_clone();
    f.set_allow_outside_depositors(&leader, 0, true)
        .expect("leader may flip allow_outside_depositors");
    assert!(f.vault(0).allow_outside_depositors.get());

    // And back to false.
    f.set_allow_outside_depositors(&leader, 0, false)
        .expect("leader may revoke");
    assert!(!f.vault(0).allow_outside_depositors.get());
    // The admin half is untouched.
    assert!(!f.vault(0).outside_deposits_approved.get());
}

#[test]
fn allow_outside_depositors_rejects_non_leader() {
    let mut f = fixture_with_vault();
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_allow_outside_depositors(&stranger, 0, true)
        .expect_err("non-leader must not flip the leader flag");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert!(!f.vault(0).allow_outside_depositors.get());
}

#[test]
fn allow_outside_depositors_rejects_invalid_idx() {
    let mut f = fixture_with_vault();
    let leader = f.authority.insecure_clone();
    let err = f
        .set_allow_outside_depositors(&leader, 99, true)
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}

#[test]
fn allow_outside_depositors_rejects_empty_sector() {
    let mut f = fixture_with_vault();
    // Vacate the (in-range) sector by zeroing its leader — the
    // free-list emptiness marker. The VaultEmpty guard must fire
    // before the leader-authorization check.
    f.poke_leader_empty(0);
    let leader = f.authority.insecure_clone();
    let err = f
        .set_allow_outside_depositors(&leader, 0, true)
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, dropset::DropsetError::VaultEmpty);
}

// ── set_outside_deposits_approved (admin-only) ───────────────────────

#[test]
fn outside_deposits_approved_admin_flips_flag() {
    let mut f = fixture_with_vault();
    assert!(!f.vault(0).outside_deposits_approved.get());

    let admin = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("admin may approve outside deposits");
    assert!(f.vault(0).outside_deposits_approved.get());

    f.set_outside_deposits_approved(&admin, 0, false)
        .expect("admin may revoke approval");
    assert!(!f.vault(0).outside_deposits_approved.get());
    // The leader half is untouched.
    assert!(!f.vault(0).allow_outside_depositors.get());
}

#[test]
fn outside_deposits_approved_rejects_non_admin() {
    let mut f = fixture_with_vault();
    // The vault leader is NOT a registry admin's stand-in here: the
    // leader is `authority`, who *is* the genesis admin, so use a
    // brand-new key that is neither.
    let stranger = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .set_outside_deposits_approved(&stranger, 0, true)
        .expect_err("non-admin must not approve outside deposits");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert!(!f.vault(0).outside_deposits_approved.get());
}

#[test]
fn outside_deposits_approved_rejects_invalid_idx() {
    let mut f = fixture_with_vault();
    let admin = f.authority.insecure_clone();
    let err = f
        .set_outside_deposits_approved(&admin, 99, true)
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);
}

#[test]
fn outside_deposits_approved_rejects_empty_sector() {
    let mut f = fixture_with_vault();
    f.poke_leader_empty(0);
    let admin = f.authority.insecure_clone();
    let err = f
        .set_outside_deposits_approved(&admin, 0, true)
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, dropset::DropsetError::VaultEmpty);
}
