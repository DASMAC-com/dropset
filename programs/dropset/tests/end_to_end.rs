//! End-to-end integration tests over the full pipeline, built on the
//! shared [`Fixture`]. After each mutating step we assert the spec's
//! two key invariants:
//!
//! 1. Treasury matches the sum of vault inventory:
//!    `base_treasury.amount == Σ vault.base_atoms` (and quote).
//! 2. Share invariant I6:
//!    `total_shares == leader_shares + Σ VaultDepositor.shares`.
//!
//! `end_to_end_single_leader_pipeline` covers the leader-only path
//! (init → register_market → register_vault → set_reference_price →
//! set_liquidity_profile → deposit_leader → withdraw_leader);
//! `outside_depositor_full_lifecycle` adds the two-key gate, outside
//! deposit, and the PDA-closing withdraw.

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;

#[test]
fn end_to_end_single_leader_pipeline() {
    // `Fixture::seeded` runs init → register_market → register_vault →
    // set_reference_price → set_liquidity_profile → deposit_leader.
    let mut f = Fixture::seeded(1_000_000, 1_085_000);

    let v = f.vault(0);
    // Treasury holds exactly the seeded inventory.
    assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());
    // I6: with no outside depositors, the leader owns every share.
    assert_eq!(
        v.total_shares.get(),
        v.leader_shares.get(),
        "I6: leader owns all shares pre-withdraw"
    );
    assert!(v.total_shares.get() > 0, "seeded with shares");

    // Leader withdraws half their stake.
    let half = v.leader_shares.get() / 2;
    f.withdraw_leader(0, half, 0, 0)
        .expect("leader partial withdraw");

    let v2 = f.vault(0);
    assert_eq!(
        v2.leader_shares.get(),
        v.leader_shares.get() - half,
        "leader stake burned by the withdrawn amount"
    );
    assert_eq!(
        v2.total_shares.get(),
        v2.leader_shares.get(),
        "I6 still holds after the withdraw"
    );
    assert!(
        v2.base_atoms.get() < v.base_atoms.get(),
        "inventory drained by the withdraw"
    );
    // Treasury invariant holds post-withdraw.
    assert_eq!(f.token_balance(&f.base_treasury), v2.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v2.quote_atoms.get());
}

#[test]
fn outside_depositor_full_lifecycle() {
    // Seeded vault: leader == authority, 1.0850 ref, full ladder,
    // 1_000_000 base / 1_085_000 quote. Both outside flags start false.
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 0);

    // Two-key gate: admin approval + leader opt-in. Until both land,
    // an outside deposit is rejected.
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();

    let alice = f.funded_depositor(200_000, 200_000);
    // Distinct `base_in` per attempt so the failed-then-retried
    // transactions aren't byte-identical (LiteSVM dedups identical
    // signatures as AlreadyProcessed).
    //
    // Both flags false — the handler checks the leader opt-in first.
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("outside deposit blocked before the leader opts in");
    common::assert_program_error(&err, dropset::DropsetError::OutsideDepositorsNotAllowed);

    // Leader opts in but admin has not approved yet → NotApproved.
    f.set_allow_outside_depositors(&leader, 0, true)
        .expect("leader opts in");
    let err = f
        .deposit(&alice, 0, 50_001, 0, 200_000, 200_000)
        .expect_err("still blocked: admin has not approved");
    common::assert_program_error(&err, dropset::DropsetError::OutsideDepositorsNotApproved);

    // Admin approves — both halves of the gate now satisfied.
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("admin approves");

    // Now the outside deposit lands. Size the base leg; the basket
    // pulls the proportional quote leg too.
    f.deposit(&alice, 0, 50_002, 0, 200_000, 200_000)
        .expect("outside deposit");
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        1,
        "deposit opened exactly one VaultDepositor"
    );
    let vd = f
        .vault_depositor(0, &alice.pubkey())
        .expect("VaultDepositor PDA exists after deposit");
    assert!(vd.shares.get() > 0, "shares minted");
    assert!(vd.net_deposits.get() > 0, "basis stamped");
    assert!(vd.gross_deposited.get() > 0, "gross stamped");
    assert_ne!(vd.entry_ref_price.as_u32(), 0, "entry_ref_price stamped");

    // Treasury invariant holds after the deposit.
    let v = f.vault(0);
    assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());

    // Partial withdraw — PDA stays open, counter unchanged.
    let half = vd.shares.get() / 2;
    assert!(half > 0);
    f.withdraw(&alice, 0, half, 0, 0).expect("partial withdraw");
    let vd2 = f
        .vault_depositor(0, &alice.pubkey())
        .expect("PDA still open after partial withdraw");
    assert_eq!(vd2.shares.get(), vd.shares.get() - half);
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        1,
        "partial withdraw does not close the PDA"
    );

    // Full exit — PDA closed, counter back to zero.
    f.withdraw(&alice, 0, vd2.shares.get(), 0, 0)
        .expect("full withdraw");
    assert!(
        f.vault_depositor(0, &alice.pubkey()).is_none(),
        "VaultDepositor PDA closed on zero shares"
    );
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        0,
        "counter decremented on close"
    );

    // Treasury invariant still holds after both withdrawals.
    let v = f.vault(0);
    assert_eq!(f.token_balance(&f.base_treasury), v.base_atoms.get());
    assert_eq!(f.token_balance(&f.quote_treasury), v.quote_atoms.get());
}
