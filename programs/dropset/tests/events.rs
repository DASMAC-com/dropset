//! Event-emission coverage (ENG-447). Every cold-path instruction emits
//! its structured event via `emit_cpi!`; before this file the suite
//! asserted none of them. Here we drive each emitter and decode the
//! event off the transaction's inner instructions (see
//! [`common::events`]), asserting the field values match the on-chain
//! state the instruction just produced.
//!
//! These are the always-on events — `CreateVault`, `Deposit`, `Withdraw`,
//! `Realize`, and `Fill` plus the ENG-433-new `CloseVault` / `Freeze` —
//! so this file is not gated behind `admin-teardown`. The teardown
//! force-withdraw paths re-emit `WithdrawEvent` (same struct, asserted
//! here via the signed paths).

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::events;
use common::fixture::{simple_profile, Fixture};
use dropset::{Price, RealizeEvent};
use solana_pubkey::Pubkey;

/// Open one outside-deposit-enabled vault (sector 0) led by a distinct
/// keypair, seeded and gated open — but *without* the outside deposit,
/// so the test owns that call and its metadata. Returns `(f, leader)`.
fn open_gated_vault() -> (Fixture, Keypair) {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(0, leader.pubkey(), true, leader.pubkey())
        .expect("create_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("approve");
    (f, leader)
}

#[test]
fn create_vault_emits_create_vault_event() {
    let mut f = Fixture::bootstrap();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    let quote_authority = Pubkey::new_unique();
    let meta = f
        .create_vault_meta(50_000, quote_authority, true, leader.pubkey())
        .expect("create_vault");

    let ev = events::create_vault(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.leader, leader.pubkey().to_bytes());
    assert_eq!(ev.quote_authority, quote_authority.to_bytes());
    assert_eq!(ev.perf_fee_rate, 50_000);
    assert!(
        ev.allow_outside_depositors,
        "opened with outside deposits on"
    );
}

#[test]
fn deposit_leader_seed_emits_deposit_event() {
    let mut f = Fixture::bootstrap();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(0, leader.pubkey(), false, leader.pubkey())
        .expect("create_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");

    let meta = f
        .deposit_leader_as_meta(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");

    let ev = events::deposit(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.depositor, leader.pubkey().to_bytes());
    assert!(ev.is_leader, "leader path");
    assert!(ev.is_seeding, "first deposit is the seed");
    assert!(ev.shares_out > 0, "seed mints shares");
    // Seed: the leader owns every share.
    assert_eq!(ev.total_shares_after, ev.shares_out);
    assert_eq!(ev.leader_shares_after, ev.shares_out);
    // Event mirrors the post-deposit vault inventory.
    let v = f.vault(0);
    assert_eq!(ev.base_atoms_after, v.base_atoms.get());
    assert_eq!(ev.quote_atoms_after, v.quote_atoms.get());
    assert_eq!(ev.total_shares_after, v.total_shares.get());
    // A fresh seed sets HWM := VPS, so no perf fee accrues.
    assert_eq!(
        events::count::<RealizeEvent>(&meta),
        0,
        "no realize on seed"
    );
}

#[test]
fn outside_deposit_emits_deposit_event() {
    let (mut f, _leader) = open_gated_vault();
    let alice = f.funded_depositor(200_000, 200_000);
    let meta = f
        .deposit_meta(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    let ev = events::deposit(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.depositor, alice.pubkey().to_bytes());
    assert!(!ev.is_leader, "outside path");
    assert!(!ev.is_seeding, "not the seed");
    assert!(ev.shares_out > 0, "outside deposit mints shares");
    let vd = f
        .vault_depositor(0, &alice.pubkey())
        .expect("PDA opened by the deposit");
    assert_eq!(ev.shares_out, vd.shares.get(), "event shares match the PDA");
}

#[test]
fn outside_withdraw_emits_withdraw_event() {
    let (mut f, _leader) = open_gated_vault();
    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");
    let shares = f
        .vault_depositor(0, &alice.pubkey())
        .expect("PDA")
        .shares
        .get();

    // Full exit, so the event reports the closing burn.
    let meta = f
        .withdraw_meta(&alice, 0, shares, 0, 0)
        .expect("outside withdraw");

    let ev = events::withdraw(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.depositor, alice.pubkey().to_bytes());
    assert!(!ev.is_leader, "outside path");
    assert_eq!(ev.shares_in, shares);
    assert!(ev.base_out > 0 || ev.quote_out > 0, "basket returned");
    let v = f.vault(0);
    assert_eq!(ev.total_shares_after, v.total_shares.get());
    assert_eq!(ev.leader_shares_after, v.leader_shares.get());
}

#[test]
fn leader_withdraw_emits_withdraw_event() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let leader = f.authority.insecure_clone();
    let half = f.vault(0).leader_shares.get() / 2;
    assert!(half > 0);

    let meta = f
        .withdraw_leader_as_meta(&leader, 0, half, 0, 0)
        .expect("leader withdraw");

    let ev = events::withdraw(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.depositor, leader.pubkey().to_bytes());
    assert!(ev.is_leader, "leader path");
    assert_eq!(ev.shares_in, half);
    let v = f.vault(0);
    assert_eq!(ev.leader_shares_after, v.leader_shares.get());
    assert_eq!(ev.total_shares_after, v.total_shares.get());
}

#[test]
fn swap_emits_fill_events() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let taker = f.funded_depositor(0, 200_000);

    // Taker buys base with quote — fills against the ask side (side 0).
    let meta = f
        .swap_meta(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("swap Buy");

    let fills = events::fills(&meta);
    assert!(
        !fills.is_empty(),
        "a filled swap emits at least one FillEvent"
    );
    for fill in &fills {
        assert_eq!(fill.market, f.market.to_bytes().into());
        assert_eq!(fill.taker, taker.pubkey().to_bytes().into());
        assert_eq!(fill.side, 0, "taker Buy fills the ask side");
        assert_eq!(fill.sector_idx, 0);
        assert!(fill.fill_base > 0 && fill.fill_quote > 0, "non-zero fill");
    }
}

#[test]
fn deposit_realizes_perf_fee_and_emits_realize_event() {
    let mut f = Fixture::bootstrap();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    // 10% perf fee so a NAV gain mints a visible fee.
    f.create_vault(100_000, leader.pubkey(), false, leader.pubkey())
        .expect("create_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");

    // Drop the HWM well below the seeded value-per-share so the next
    // realize sees a gain and accrues a perf fee. (No swap plumbing
    // needed — HWM lagging behind NAV is the exact accrual condition.)
    f.poke_hwm(0, dropset::Q32_32_ONE / 2);
    let leader_shares_before = f.vault(0).leader_shares.get();

    // Advance the blockhash so the top-up's funding mints aren't
    // byte-identical to the seed's (same mint, same amount, same payer)
    // — LiteSVM would otherwise reject the duplicate as AlreadyProcessed.
    f.svm.expire_blockhash();

    // A tiny leader top-up triggers realize *before* the deposit shares
    // are added, so the RealizeEvent is emitted alongside the Deposit.
    let meta = f
        .deposit_leader_as_meta(&leader, 0, 1, 0, 1_000_000, 1_000_000)
        .expect("top-up");

    let ev = events::realize(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert!(ev.shares_minted > 0, "perf fee minted shares");
    assert!(
        ev.leader_shares_after > leader_shares_before,
        "minted fee shares went to the leader"
    );
    assert!(
        ev.hwm_after > dropset::Q32_32_ONE / 2,
        "HWM bumped up toward the realized VPS"
    );
}

#[test]
fn close_vault_emits_close_vault_event() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let leader = f.authority.insecure_clone();
    let meta = f.close_vault_meta(&leader, 0).expect("close_vault");

    let ev = events::close_vault(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.leader, leader.pubkey().to_bytes());
    assert_eq!(
        ev.active_count_after, 0,
        "the only active vault was tombstoned"
    );
}

#[test]
fn freeze_vault_emits_freeze_vault_event() {
    let mut f = Fixture::seeded(1_000_000, 1_085_000);
    let admin = f.authority.insecure_clone();
    let meta = f.freeze_vault_meta(&admin, 0).expect("freeze_vault");

    let ev = events::freeze_vault(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    // Seeded vault: the leader is the admin/authority.
    assert_eq!(ev.leader, admin.pubkey().to_bytes());
}
