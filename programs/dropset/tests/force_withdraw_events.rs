//! Event coverage for the admin force-withdraw paths (the `admin-teardown`
//! feature surface). `events.rs` decodes the signed withdraw paths; these
//! decode the three force-withdraw emit paths in `force_withdraw.rs`:
//!
//! - `force_withdraw_depositor` — a `WithdrawEvent` plus the `RealizeEvent`
//!   when its realize step mints;
//! - `force_withdraw_leader` on a zero-stake vault — the early return that
//!   reclaims the sector and emits a zero-valued `WithdrawEvent`, no
//!   realize;
//! - `force_withdraw_leader` on a funded vault — the full drain.
//!
//! The vault under test is opened on a sector whose first occupant was
//! already reclaimed, so its `vault_seq` is 2: a path that captured the
//! sector index, or a stale seq, would not pass by coincidence.

#![cfg(feature = "admin-teardown")]

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::events;
use common::fixture::{simple_profile, Fixture};
use dropset::{Price, RealizeEvent};

/// The second vault to occupy sector 0, so `seq == 2`.
const VAULT_SEQ: u64 = 2;

/// Bootstrap → an empty vault on sector 0, reclaimed by the zero-stake
/// force-withdraw → a second vault on the same sector with a 10% perf fee,
/// seeded by `leader` and holding an outside position for `alice`. The HWM
/// is then dropped below the value per share so the next realize mints a
/// fee. Returns `(f, admin, leader, alice)`.
fn funded_vault_on_a_reused_sector() -> (Fixture, Keypair, Keypair, Keypair) {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);

    f.create_vault(0, leader.pubkey(), true, leader.pubkey())
        .expect("first occupant");
    f.force_withdraw_leader(&admin, 0, &leader.pubkey())
        .expect("reclaim the empty first occupant");

    f.create_vault(100_000, leader.pubkey(), true, leader.pubkey())
        .expect("second occupant");
    assert_eq!(f.vault(0).seq.get(), VAULT_SEQ);
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("approve");
    let alice = f.funded_depositor(200_000, 200_000);
    f.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
        .expect("outside deposit");

    f.poke_hwm(0, dropset::Q32_32_ONE / 2);
    // The leader force-withdraw below would otherwise be byte-identical to
    // the reclaim above, and LiteSVM rejects the duplicate.
    f.svm.expire_blockhash();
    (f, admin, leader, alice)
}

#[test]
fn force_withdraw_depositor_emits_withdraw_and_realize() {
    let (mut f, admin, _leader, alice) = funded_vault_on_a_reused_sector();
    let owner = alice.pubkey();
    let shares = f.vault_depositor(0, &owner).expect("PDA").shares.get();
    let (base_ata, quote_ata) = (f.base_ata(&owner), f.quote_ata(&owner));
    let (base_before, quote_before) = (f.token_balance(&base_ata), f.token_balance(&quote_ata));

    let meta = f
        .force_withdraw_depositor_meta(&admin, 0, &owner)
        .expect("force_withdraw_depositor");
    let v = f.vault(0);

    let w = events::withdraw(&meta);
    assert_eq!(w.market, f.market.to_bytes());
    assert_eq!(w.sector_idx, 0);
    assert_eq!(w.vault_seq, VAULT_SEQ);
    assert_eq!(w.depositor, owner.to_bytes());
    assert!(!w.is_leader, "outside path");
    assert_eq!(w.shares_in, shares, "the whole position");
    assert_eq!(w.base_out, f.token_balance(&base_ata) - base_before);
    assert_eq!(w.quote_out, f.token_balance(&quote_ata) - quote_before);
    assert!(w.base_out > 0 || w.quote_out > 0, "basket returned");
    assert_eq!(w.total_shares_after, v.total_shares.get());
    assert_eq!(w.leader_shares_after, v.leader_shares.get());
    assert_eq!(w.base_atoms_after, v.base_atoms.get());
    assert_eq!(w.quote_atoms_after, v.quote_atoms.get());

    let r = events::realize(&meta);
    assert_eq!(r.market, f.market.to_bytes());
    assert_eq!(r.sector_idx, 0);
    assert_eq!(r.vault_seq, VAULT_SEQ);
    assert!(r.shares_minted > 0, "the lowered HWM accrues a perf fee");
    assert_eq!(r.leader_shares_after, v.leader_shares.get());
    assert_eq!(r.total_shares_after, v.total_shares.get());
}

#[test]
fn force_withdraw_leader_on_an_empty_vault_emits_a_zero_withdraw() {
    let mut f = Fixture::bootstrap();
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(0, leader.pubkey(), true, leader.pubkey())
        .expect("admin opens an empty vault");
    let seq = f.vault(0).seq.get();

    let meta = f
        .force_withdraw_leader_meta(&admin, 0, &leader.pubkey())
        .expect("zero-stake reclaim");

    let w = events::withdraw(&meta);
    assert_eq!(w.market, f.market.to_bytes());
    assert_eq!(w.sector_idx, 0);
    assert_eq!(w.vault_seq, seq);
    assert_eq!(w.depositor, leader.pubkey().to_bytes());
    assert!(w.is_leader, "leader path");
    assert_eq!(
        (w.shares_in, w.base_out, w.quote_out),
        (0, 0, 0),
        "nothing drained"
    );
    assert_eq!(
        (w.total_shares_after, w.leader_shares_after),
        (0, 0),
        "no shares before or after"
    );
    assert_eq!((w.base_atoms_after, w.quote_atoms_after), (0, 0));
    assert_eq!(w.realized_pnl_delta, 0);
    assert_eq!(
        events::count::<RealizeEvent>(&meta),
        0,
        "the early return skips realize"
    );
}

#[test]
fn force_withdraw_leader_drain_emits_withdraw_and_realize() {
    let (mut f, admin, leader, alice) = funded_vault_on_a_reused_sector();
    // Depositors first, per the teardown order, so the leader drain zeroes
    // the vault. Drop the HWM again so the leader's own realize mints.
    f.force_withdraw_depositor(&admin, 0, &alice.pubkey())
        .expect("force_withdraw_depositor");
    f.poke_hwm(0, dropset::Q32_32_ONE / 2);
    let owner = leader.pubkey();
    let leader_shares_before = f.vault(0).leader_shares.get();
    let (base_ata, quote_ata) = (f.base_ata(&owner), f.quote_ata(&owner));
    let (base_before, quote_before) = (f.token_balance(&base_ata), f.token_balance(&quote_ata));

    let meta = f
        .force_withdraw_leader_meta(&admin, 0, &owner)
        .expect("force_withdraw_leader");

    let r = events::realize(&meta);
    assert_eq!(r.market, f.market.to_bytes());
    assert_eq!(r.sector_idx, 0);
    assert_eq!(r.vault_seq, VAULT_SEQ);
    assert!(r.shares_minted > 0, "the lowered HWM accrues a perf fee");
    // Reported after the drain, which reclaims the sector.
    assert_eq!((r.leader_shares_after, r.total_shares_after), (0, 0));

    let w = events::withdraw(&meta);
    assert_eq!(w.market, f.market.to_bytes());
    assert_eq!(w.sector_idx, 0);
    assert_eq!(w.vault_seq, VAULT_SEQ);
    assert_eq!(w.depositor, owner.to_bytes());
    assert!(w.is_leader, "leader path");
    assert_eq!(
        w.shares_in,
        leader_shares_before + r.shares_minted,
        "the whole stake, fee shares included"
    );
    assert_eq!(w.base_out, f.token_balance(&base_ata) - base_before);
    assert_eq!(w.quote_out, f.token_balance(&quote_ata) - quote_before);
    assert_eq!((w.total_shares_after, w.leader_shares_after), (0, 0));
    assert_eq!((w.base_atoms_after, w.quote_atoms_after), (0, 0));
    assert_eq!(w.realized_pnl_delta, 0);
    assert_eq!(
        f.market_header().free_head.get(),
        0,
        "the drain reclaimed sector 0"
    );
}
