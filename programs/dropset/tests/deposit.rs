// cspell:word topoff
//! `deposit` integration tests — the outside-depositor PDA path.
//! Covers the happy open (PDA + basis stamp + counter), the
//! shares-weighted top-off merge, and every rejection gate
//! (leader-signer, unseeded, two-key flags, missing reference price,
//! slippage, leader-share floor).

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;
use dropset::Price;
use solana_pubkey::Pubkey;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;

/// Seeded vault with both outside-deposit flags enabled.
fn seeded_open() -> Fixture {
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();
    f
}

#[test]
fn happy_path_opens_pda_stamps_basis_and_bumps_counter() {
    let mut f = seeded_open();
    let alice = f.funded_depositor(200_000, 200_000);
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 0);

    f.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    assert_eq!(
        f.market_header().outstanding_vault_depositors.get(),
        1,
        "counter incremented by exactly 1"
    );
    let vd = f.vault_depositor(0, &alice.pubkey()).expect("PDA created");
    assert!(vd.shares.get() > 0, "shares stamped");
    assert!(vd.net_deposits.get() > 0, "net_deposits stamped");
    assert!(vd.gross_deposited.get() > 0, "gross_deposited stamped");
    assert_ne!(vd.entry_ref_price.as_u32(), 0, "entry_ref_price stamped");
    assert!(vd.entry_vps.get() > 0, "entry_vps stamped");
}

#[test]
fn topoff_merges_entry_ref_price() {
    let mut f = seeded_open();
    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, 50_000, 0, 400_000, 400_000)
        .expect("first deposit");
    let vd1 = f.vault_depositor(0, &alice.pubkey()).unwrap();
    let entry_ref_1 = vd1.entry_ref_price.as_u32();

    // Move the reference price up to 1.2000 before the top-off, so the
    // shares-weighted average must shift toward it.
    let higher = Price::encode(12_000_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, higher.as_u32(), 0)
        .expect("raise reference price");

    // Distinct leg size so the retry isn't a byte-identical txn.
    f.deposit(&alice, 0, 50_001, 0, 400_000, 400_000)
        .expect("top-off");

    let vd2 = f.vault_depositor(0, &alice.pubkey()).unwrap();
    assert!(vd2.shares.get() > vd1.shares.get(), "shares grew");
    assert!(
        vd2.entry_ref_price.as_u32() > entry_ref_1,
        "entry_ref blended upward toward the new reference"
    );
    assert!(
        vd2.entry_ref_price.as_u32() < higher.as_u32(),
        "blend stays below the new reference (old lot still weighs in)"
    );
    // Top-off into an existing PDA must NOT re-increment the counter.
    assert_eq!(f.market_header().outstanding_vault_depositors.get(), 1);
}

#[test]
fn rejects_leader_as_depositor() {
    let mut f = seeded_open();
    // The leader's own ATAs exist (from the seed) but the handler
    // rejects `signer == leader` before any transfer.
    let leader = f.authority.insecure_clone();
    let err = f
        .deposit(&leader, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("leader must use deposit_leader, not the outside path");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
}

#[test]
fn rejects_tombstoned_vault() {
    // A vault the leader has `CloseVault`'d is winding down: it no
    // longer quotes and accrues no fee, so an outside depositor cannot
    // mint into it. The tombstone gate fires before the outside-deposit
    // opt-in gates.
    let mut f = seeded_open();
    let leader = f.authority.insecure_clone();
    let alice = f.funded_depositor(200_000, 200_000);
    f.close_vault(&leader, 0)
        .expect("leader tombstones the vault");
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("tombstoned vault must reject outside deposit");
    common::assert_program_error(&err, dropset::DropsetError::VaultTombstoned);
}

#[test]
fn rejects_unseeded_vault() {
    // Register + price + enable flags, but never seed: total_shares == 0.
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), true, Pubkey::default())
        .expect("create_vault (allow_outside = true)");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, px.as_u32(), 0)
        .unwrap();
    let admin = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();

    let alice = f.funded_depositor(200_000, 200_000);
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("deposit into an unseeded vault must reject");
    common::assert_program_error(&err, dropset::DropsetError::SeedingRequiresLeader);
}

#[test]
fn rejects_when_leader_has_not_allowed() {
    // approved = true, allow = false → the leader opt-in gate fails.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let admin = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    let alice = f.funded_depositor(200_000, 200_000);
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("allow_outside_depositors == false must reject");
    common::assert_program_error(&err, dropset::DropsetError::OutsideDepositorsNotAllowed);
}

#[test]
fn rejects_when_admin_has_not_approved() {
    // allow = true, approved = false → the admin approval gate fails.
    let mut f = Fixture::seeded(SEED_BASE, SEED_QUOTE);
    let leader = f.authority.insecure_clone();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();
    let alice = f.funded_depositor(200_000, 200_000);
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("outside_deposits_approved == false must reject");
    common::assert_program_error(&err, dropset::DropsetError::OutsideDepositorsNotApproved);
}

#[test]
fn rejects_when_reference_price_zero() {
    // Seed via the leader without ever setting a reference price, then
    // open the gate. The depositor's basis would anchor to the ZERO
    // sentinel, so the deposit is rejected.
    let mut f = Fixture::bootstrap();
    f.create_vault(0, f.authority.pubkey(), true, Pubkey::default())
        .expect("create_vault");
    f.deposit_leader(0, SEED_BASE, SEED_QUOTE, SEED_BASE, SEED_QUOTE)
        .expect("seed without a reference price");
    let admin = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();

    let alice = f.funded_depositor(200_000, 200_000);
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("deposit with an unset reference price must reject");
    common::assert_program_error(&err, dropset::DropsetError::ReferencePriceNotSet);
}

#[test]
fn rejects_basket_slippage() {
    let mut f = seeded_open();
    let alice = f.funded_depositor(200_000, 200_000);
    // Size 50_000 base but cap max_base_in at 10 — the derived basket
    // blows past the cap.
    let err = f
        .deposit(&alice, 0, 50_000, 0, 10, 200_000)
        .expect_err("basket over the slippage cap must reject");
    common::assert_program_error(&err, dropset::DropsetError::BasketSlippage);
}

#[test]
fn rejects_min_leader_share_violation() {
    let mut f = seeded_open();
    // Pin the floor at 99% — any meaningful outside deposit drops the
    // leader's ratio below it.
    f.poke_min_leader_share(0, 990_000);
    let alice = f.funded_depositor(200_000, 200_000);
    let err = f
        .deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect_err("deposit pushing leader share below the floor must reject");
    common::assert_program_error(&err, dropset::DropsetError::MinLeaderShareViolated);
}
