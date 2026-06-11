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

    let vd2 = f.vault_depositor(0, &alice.pubkey()).expect("PDA still open");
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
