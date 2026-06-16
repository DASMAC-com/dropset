//! `withdraw` integration tests — the outside-depositor exit path.
//! Covers partial exit (basis reduction, PDA stays open), full exit
//! (PDA close + counter decrement), the share-balance / slippage
//! rejections, and realized-FX crystallization across a reference-price
//! move.
//!
//! Note on the `signer == leader` rejection: it is a defensive check in
//! the handler, but is unreachable through normal flow — `deposit`
//! rejects the leader, so a leader can never own a `VaultDepositor`
//! PDA to pass into `withdraw` in the first place. Reaching it would
//! require fabricating a leader-owned PDA, so it is not exercised here.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::Fixture;
use dropset::Price;
use solana_pubkey::Pubkey;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

/// Seeded + outside-enabled vault with one outside depositor `alice`
/// holding a position from a `base_in`-sized deposit.
fn open_with_depositor(base_in: u64) -> (Fixture, Keypair) {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();
    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, base_in, 0, 400_000, 400_000)
        .expect("outside deposit");
    (f, alice)
}

#[test]
fn partial_withdraw_reduces_basis_and_keeps_pda() {
    let (mut f, alice) = open_with_depositor(100_000);
    let vd = f.vault_depositor(0, &alice.pubkey()).unwrap();
    let half = vd.shares.get() / 2;
    assert!(half > 0);

    f.withdraw(&alice, 0, half, 0, 0).expect("partial withdraw");

    let vd2 = f
        .vault_depositor(0, &alice.pubkey())
        .expect("PDA still open");
    assert_eq!(vd2.shares.get(), vd.shares.get() - half, "shares burned");
    assert!(
        vd2.net_deposits.get() < vd.net_deposits.get(),
        "net_deposits reduced by the released-basis slice"
    );
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        1,
        "partial exit leaves the PDA (and counter) in place"
    );
}

#[test]
fn full_exit_closes_pda_and_decrements_counter() {
    let (mut f, alice) = open_with_depositor(100_000);
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 1);

    f.withdraw(&alice, 0, shares, 0, 0).expect("full withdraw");

    assert!(
        f.vault_depositor(0, &alice.pubkey()).is_none(),
        "PDA closed at zero shares"
    );
    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        0,
        "counter decremented on close"
    );
}

#[test]
fn last_outside_exit_on_winding_down_vault_reclaims_sector() {
    // The headline ENG-462 path: an admin freezes the vault, the leader
    // fully exits via `withdraw_leader` (the min-leader-share floor is
    // bypassed once frozen), leaving outside shares behind with
    // `leader_shares == 0`. The last outside depositor's exit then drives
    // `total_shares` to 0, which must reclaim the sector to the free DLL.
    // Before the fix the signed `withdraw` path never reclaimed, leaking
    // the slab slot and the `active_count` it held.
    let (mut f, alice) = open_with_depositor(100_000);
    let admin = f.authority.insecure_clone();
    assert_eq!(f.market_header().active_count.get(), 1);

    // Freeze, then drain the leader to zero (floor bypassed while frozen).
    f.freeze_vault(&admin, 0).expect("freeze vault");
    let leader_shares = f.vault(0).leader_shares.get();
    f.withdraw_leader(0, leader_shares, 0, 0)
        .expect("leader exits fully on the frozen vault");
    assert_eq!(f.vault(0).leader_shares.get(), 0, "leader fully out");
    assert!(
        f.vault(0).total_shares.get() > 0,
        "outside shares still hold the vault open"
    );
    assert_eq!(
        f.market_header().active_count.get(),
        1,
        "not yet reclaimed — outside shares remain"
    );

    // The last outside depositor drains the remainder → reclaim.
    let alice_shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    f.withdraw(&alice, 0, alice_shares, 0, 0)
        .expect("last outside exit drains the vault");

    let v = f.vault(0);
    assert_eq!(v.total_shares.get(), 0, "vault fully drained");
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
    assert!(
        f.vault_depositor(0, &alice.pubkey()).is_none(),
        "depositor PDA closed at zero shares"
    );
    assert_eq!(
        h.outstanding_vault_depositors.get(),
        0,
        "depositor counter back to zero"
    );
}

#[test]
fn rejects_shares_over_balance() {
    let (mut f, alice) = open_with_depositor(100_000);
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    let err = f
        .withdraw(&alice, 0, shares + 1, 0, 0)
        .expect_err("shares_in over balance must reject");
    common::assert_program_error(&err, dropset::DropsetError::InsufficientShares);
}

#[test]
fn rejects_zero_shares() {
    let (mut f, alice) = open_with_depositor(100_000);
    let err = f
        .withdraw(&alice, 0, 0, 0, 0)
        .expect_err("zero shares must reject");
    common::assert_program_error(&err, dropset::DropsetError::InsufficientShares);
}

#[test]
fn rejects_basket_slippage() {
    let (mut f, alice) = open_with_depositor(100_000);
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    // Demand far more base out than the pro-rata slice can deliver.
    let err = f
        .withdraw(&alice, 0, shares / 2, u64::MAX, 0)
        .expect_err("min_base_out over the slice must reject");
    common::assert_program_error(&err, dropset::DropsetError::BasketSlippage);
}

#[test]
fn realized_fx_positive_when_reference_rises() {
    let (mut f, alice) = open_with_depositor(100_000);
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();

    // Reference rises 1.0850 → 1.2000 between deposit and withdraw, so
    // the base leg appreciated in quote terms: realized_fx > 0.
    let higher = Price::encode(12_000_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, higher.as_u32(), 0)
        .expect("raise reference price");

    f.withdraw(&alice, 0, shares / 2, 0, 0)
        .expect("partial withdraw after price rise");

    let vd = f.vault_depositor(0, &alice.pubkey()).unwrap();
    assert!(
        vd.realized_fx.get() > 0,
        "rising reference crystallizes positive FX PnL (got {})",
        vd.realized_fx.get()
    );
    // Invariant: realized_yield + realized_fx == realized_pnl.
    assert_eq!(
        vd.realized_yield.get() + vd.realized_fx.get(),
        vd.realized_pnl.get(),
        "yield + fx == pnl"
    );
}
