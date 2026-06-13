//! Steady-state vault-lifecycle instruction tests: `close_vault`
//! (leader moves a vault active → tombstone) and `freeze_vault` (admin
//! freezes a vault in place). These are always-on instructions —
//! independent of the `admin-teardown` feature — so this file is not
//! feature-gated.
//!
//! The frozen-vault *rejection* paths (set_reference_price /
//! set_liquidity_profile / deposit_leader / swap declining a frozen
//! vault) live in those instructions' own test files; here we assert the
//! freeze itself — the flag write, the admin gate — and the close
//! transition end to end.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;

#[test]
fn close_vault_moves_active_to_tombstone() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let leader = f.authority.insecure_clone();

    // Precondition: one active vault at sector 0, nothing tombstoned.
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 1);
    assert_eq!(h.head.get(), 0, "vault 0 heads the active list");
    assert_eq!(h.tombstone_head.get(), dropset::NULL_SECTOR);

    let shares_before = f.vault(0).total_shares.get();
    assert!(!f.vault(0).tombstoned.get(), "vault starts un-tombstoned");
    f.close_vault(&leader, 0).expect("leader closes the vault");

    let h = f.market_header();
    assert_eq!(h.active_count.get(), 0, "active count decremented");
    assert_eq!(h.head.get(), dropset::NULL_SECTOR, "active list now empty");
    assert_eq!(h.tombstone_head.get(), 0, "vault 0 moved to tombstone");
    assert!(
        f.vault(0).tombstoned.get(),
        "tombstoned flag set so handlers can read it without a list walk"
    );

    // The sector keeps its data — depositor flows stay open until it
    // drains; only the list membership changed.
    let v = f.vault(0);
    assert_eq!(
        v.leader,
        leader.pubkey().to_bytes().into(),
        "leader preserved on tombstone"
    );
    assert_eq!(
        v.total_shares.get(),
        shares_before,
        "shares preserved on tombstone"
    );
}

#[test]
fn close_vault_rejects_non_leader() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .close_vault(&stranger, 0)
        .expect_err("only the leader may close their vault");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn freeze_vault_sets_frozen_flag() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    assert!(!f.vault(0).frozen.get(), "vault starts unfrozen");

    f.freeze_vault(&admin, 0).expect("admin freezes the vault");
    assert!(f.vault(0).frozen.get(), "frozen flag set");

    // The vault stays on the active DLL (existing levels keep matching
    // until expiry) — freeze is not a list move.
    let h = f.market_header();
    assert_eq!(h.active_count.get(), 1, "frozen vault still counted active");
    assert_eq!(h.head.get(), 0, "frozen vault still on the active list");
}

#[test]
fn freeze_vault_rejects_non_admin() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    // The vault's own leader is not an admin lever here — only a registry
    // admin may freeze. Use a stranger to prove the admin gate.
    let stranger = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let err = f
        .freeze_vault(&stranger, 0)
        .expect_err("only a registry admin may freeze a vault");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn close_vault_rejects_already_tombstoned() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let leader = f.authority.insecure_clone();
    f.close_vault(&leader, 0)
        .expect("first close moves the vault to the tombstone list");
    // A second close must reject as `VaultAlreadyTombstoned`. Advance the
    // blockhash so the re-sent transaction isn't a byte-identical
    // duplicate — LiteSVM dedups those as `AlreadyProcessed` before the
    // program ever runs, which would mask the program-level rejection.
    f.svm.expire_blockhash();
    let err = f
        .close_vault(&leader, 0)
        .expect_err("a tombstoned vault cannot be closed again");
    common::assert_program_error(&err, dropset::DropsetError::VaultAlreadyTombstoned);
}
