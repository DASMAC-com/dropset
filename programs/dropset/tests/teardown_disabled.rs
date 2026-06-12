//! Feature-off coverage (ENG-447): with `admin-teardown` compiled out
//! — the shape of the final immutable deploy — every teardown
//! instruction's dispatcher short-circuits to `DropsetError::TeardownDisabled`.
//!
//! The guard lives in the `#[cfg(not(feature = "admin-teardown"))]` arm
//! of each handler, so it is only reachable in a feature-off build. This
//! target is therefore gated to that build and is a no-op under the
//! default-feature `make test`; `make test-no-teardown` builds the
//! matching feature-off `.so` and runs it (see the `Tests (no teardown)`
//! CI job).
//!
//! Account validation still runs before the handler, so each call is set
//! up with accounts that resolve (a live depositor PDA, existing ATAs);
//! the rejection then comes from the handler guard, not a missing
//! account.

#![cfg(not(feature = "admin-teardown"))]

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;

fn assert_disabled(err: &str) {
    common::assert_program_error(err, dropset::DropsetError::TeardownDisabled);
}

#[test]
fn force_withdraw_depositor_disabled() {
    let mut f = Fixture::bootstrap();
    let (_leader, alice) = f.with_outside_depositor();
    let admin = f.authority.insecure_clone();
    let err = f
        .force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect_err("force_withdraw_depositor is compiled out");
    assert_disabled(&err);
}

#[test]
fn force_withdraw_leader_disabled() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    // Distinct admin so it never aliases the leader (= authority) account.
    let admin = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let leader = f.authority.pubkey();
    let err = f
        .force_withdraw_leader(&admin, 0, &leader)
        .expect_err("force_withdraw_leader is compiled out");
    assert_disabled(&err);
}

#[test]
fn close_market_treasury_disabled() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let (base_mint, base_treasury) = (f.base_mint, f.base_treasury);
    let err = f
        .close_market_treasury(&admin, &base_mint, &base_treasury, &rr)
        .expect_err("close_market_treasury is compiled out");
    assert_disabled(&err);
}

#[test]
fn close_market_disabled() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_market(&admin, &rr)
        .expect_err("close_market is compiled out");
    assert_disabled(&err);
}

#[test]
fn close_registry_fee_vault_disabled() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_registry_fee_vault(&admin, &rr)
        .expect_err("close_registry_fee_vault is compiled out");
    assert_disabled(&err);
}

#[test]
fn close_registry_disabled() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let rr = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS).pubkey();
    let err = f
        .close_registry(&admin, &rr)
        .expect_err("close_registry is compiled out");
    assert_disabled(&err);
}
