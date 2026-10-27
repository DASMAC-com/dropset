//! `withdraw` integration tests — the outside-depositor exit path.
//! Covers partial exit (basis reduction, PDA stays open), full exit
//! (PDA close + counter decrement), the share-balance / slippage
//! rejections, realized-FX crystallization across a reference-price
//! move, and a withdraw after a performance fee has accrued.
//!
//! Note on the `signer == leader` rejection: it is a defensive check in
//! the handler, but is unreachable through normal flow — `deposit`
//! rejects the leader, so a leader can never own a `VaultDepositor`
//! PDA to pass into `withdraw` in the first place. Reaching it would
//! require fabricating a leader-owned PDA, so it is not exercised here.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{flip_profile, simple_profile, Fixture};
use common::{events, SIGNER_FUNDING_LAMPORTS};
use dropset::{Price, RealizeEvent};
use dropset_math_core::share::{compute_pro_rata_slice, realize_perf_fee};
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
    // The headline path: an admin freezes the vault, the leader
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
fn last_outside_exit_on_tombstoned_vault_reclaims_sector() {
    // Tombstone variant of the reclaim path — the active-list case is
    // covered by `last_outside_exit_on_winding_down_vault_reclaims_sector`
    // (a frozen vault stays on the active DLL). Here the leader tombstones
    // the vault via `close_vault`, moving it to the *tombstone* DLL and
    // dropping `active_count` to 0; the leader then fully exits (floor
    // bypassed while tombstoned) and the last outside depositor drains the
    // remainder through signed `withdraw`. That final burn must unlink the
    // sector from the tombstone list — the `DllList::Tombstone` branch of
    // `reclaim_sector`, which (unlike the active branch) does not touch
    // `active_count` — and return it to the free list.
    let (mut f, alice) = open_with_depositor(100_000);
    let leader = f.authority.insecure_clone();

    f.close_vault(&leader, 0)
        .expect("leader tombstones the vault");
    assert_eq!(
        f.market_header().active_count.get(),
        0,
        "tombstoning drops the active count"
    );

    // Leader exits fully (floor bypassed while tombstoned); outside shares remain.
    let leader_shares = f.vault(0).leader_shares.get();
    f.withdraw_leader(0, leader_shares, 0, 0)
        .expect("leader exits fully on the tombstoned vault");
    assert!(
        f.vault(0).total_shares.get() > 0,
        "outside shares still hold the vault open"
    );

    // The last outside depositor drains the remainder → reclaim from tombstone.
    let alice_shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    f.withdraw(&alice, 0, alice_shares, 0, 0)
        .expect("last outside exit drains the tombstoned vault");

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
        "active count stays zero — already decremented at tombstone time"
    );
    assert_eq!(
        h.tombstone_head.get(),
        dropset::NULL_SECTOR,
        "tombstone list now empty"
    );
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
fn realized_price_positive_when_reference_rises() {
    let (mut f, alice) = open_with_depositor(100_000);
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();

    // Reference rises 1.0850 → 1.2000 between deposit and withdraw, so
    // the base leg appreciated in quote terms: realized_price > 0.
    let higher = Price::encode(12_000_000, 0).unwrap();
    f.set_reference_price(&f.authority.insecure_clone(), 0, higher.as_u32(), 0)
        .expect("raise reference price");

    f.withdraw(&alice, 0, shares / 2, 0, 0)
        .expect("partial withdraw after price rise");

    let vd = f.vault_depositor(0, &alice.pubkey()).unwrap();
    assert!(
        vd.realized_price.get() > 0,
        "rising reference crystallizes positive FX PnL (got {})",
        vd.realized_price.get()
    );
    // Invariant: realized_yield + realized_price == realized_pnl.
    assert_eq!(
        vd.realized_yield.get() + vd.realized_price.get(),
        vd.realized_pnl.get(),
        "yield + fx == pnl"
    );
}

/// Perf-fee vault (10%) led by a distinct keypair, seeded, opened to
/// outside deposits, with one outside depositor `alice`. When `trade` is
/// set, a taker then round-trips against the ladder — buys base at the
/// ask, sells it back at the bid — so the vault keeps both legs' spread
/// with its inventory back near the reference ratio. That lifts the
/// value per share, `isqrt(base · quote) / total_shares`, past the HWM,
/// and the next realize accrues a fee. (A single one-sided fill of this
/// size would not: pushing inventory off the ratio costs the geometric
/// mean more than the premium earns.) Returns `(leader, alice)`.
fn perf_fee_vault_with_depositor(f: &mut Fixture, trade: bool) -> (Keypair, Keypair) {
    let admin = f.authority.insecure_clone();
    let leader = f.funded_keypair(10 * SIGNER_FUNDING_LAMPORTS);
    f.create_vault(100_000, leader.pubkey(), true, leader.pubkey())
        .expect("create_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader_as(&leader, 0, SEED_BASE, SEED_QUOTE, SEED_BASE, SEED_QUOTE)
        .expect("seed");
    f.set_outside_deposits_approved(&admin, 0, true)
        .expect("admin approves");
    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, 100_000, 0, 400_000, 400_000)
        .expect("outside deposit");
    if trade {
        let taker = f.funded_depositor(0, 200_000);
        f.swap(&taker, 0, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("taker buys base at the ask premium");
        let bought = f.token_balance(&f.base_ata(&taker.pubkey()));
        f.swap(&taker, 1, bought, Price::ZERO.as_u32(), 1)
            .expect("taker sells it back at the bid discount");
    }
    (leader, alice)
}

/// Withdraw half of `alice`'s shares and assert the handler's order —
/// realize, re-read the share count, slice — against the
/// `dropset-math-core` kernels the shared share vectors pin for the TS
/// SDK (`sdk/conformance/share_vectors.json`). Returns the fee shares
/// the realize minted.
fn assert_withdraw_matches_kernels(f: &mut Fixture, alice: &Keypair) -> u64 {
    let pre = f.vault(0);
    let shares_in = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get() / 2;
    let (b_ata, q_ata) = (f.base_ata(&alice.pubkey()), f.quote_ata(&alice.pubkey()));
    let (b_before, q_before) = (f.token_balance(&b_ata), f.token_balance(&q_ata));

    let r = realize_perf_fee(
        pre.base_atoms.get(),
        pre.quote_atoms.get(),
        pre.total_shares.get(),
        pre.leader_shares.get(),
        pre.hwm.get(),
        pre.perf_fee_rate.get(),
    );
    let (want_base, want_quote) = compute_pro_rata_slice(
        shares_in,
        r.total_shares_after,
        pre.base_atoms.get(),
        pre.quote_atoms.get(),
    );

    let meta = f
        .withdraw_meta(alice, 0, shares_in, 0, 0)
        .expect("withdraw");

    // The payout is the slice against the realized share count.
    let ev = events::withdraw(&meta);
    assert_eq!((ev.base_out, ev.quote_out), (want_base, want_quote));
    assert_eq!(f.token_balance(&b_ata) - b_before, want_base);
    assert_eq!(f.token_balance(&q_ata) - q_before, want_quote);

    // The fee shares went to the leader and stay in the vault: only
    // alice's slice left the inventory and only her shares were burned.
    let post = f.vault(0);
    assert_eq!(
        post.leader_shares.get(),
        pre.leader_shares.get() + r.shares_minted
    );
    assert_eq!(post.total_shares.get(), r.total_shares_after - shares_in);
    assert_eq!(post.hwm.get(), r.hwm_after);
    assert_eq!(post.base_atoms.get(), pre.base_atoms.get() - want_base);
    assert_eq!(post.quote_atoms.get(), pre.quote_atoms.get() - want_quote);

    // A `RealizeEvent` rides along exactly when the realize minted.
    let realize_events = events::count::<RealizeEvent>(&meta);
    assert_eq!(realize_events, usize::from(r.shares_minted > 0));
    if r.shares_minted > 0 {
        let rev = events::realize(&meta);
        assert_eq!(rev.shares_minted, r.shares_minted);
        // Every `RealizeEvent` call site reports the post-instruction
        // totals, so here that is after alice's burn, not the realize's.
        assert_eq!(rev.total_shares_after, post.total_shares.get());
    }
    r.shares_minted
}

#[test]
fn withdraw_after_perf_fee_accrual_pays_the_fee_diluted_slice() {
    let mut f = Fixture::bootstrap();
    let (_leader, alice) = perf_fee_vault_with_depositor(&mut f, true);

    // Sized against the stored (pre-realize) share count, the slice would
    // hand alice part of the leader's fee.
    let pre = f.vault(0);
    let shares_in = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get() / 2;
    let (stale_base, stale_quote) = compute_pro_rata_slice(
        shares_in,
        pre.total_shares.get(),
        pre.base_atoms.get(),
        pre.quote_atoms.get(),
    );

    let minted = assert_withdraw_matches_kernels(&mut f, &alice);
    assert!(minted > 0, "the trade accrued a perf fee");

    let paid_base = pre.base_atoms.get() - f.vault(0).base_atoms.get();
    let paid_quote = pre.quote_atoms.get() - f.vault(0).quote_atoms.get();
    assert!(
        paid_base < stale_base && paid_quote < stale_quote,
        "the fee-diluted payout is below the stale-count slice"
    );
}

#[test]
fn withdraw_without_accrual_pays_the_stored_count_slice() {
    let mut f = Fixture::bootstrap();
    let (_leader, alice) = perf_fee_vault_with_depositor(&mut f, false);

    let minted = assert_withdraw_matches_kernels(&mut f, &alice);
    // The outside deposit's floored share count can leave value per share
    // a few quanta above the HWM; that gain is sub-quantum, so no fee.
    assert_eq!(minted, 0, "no trade, no fee");
}

#[test]
fn resting_sizes_track_inventory_across_deposit_and_withdraw() {
    // One 20% level per side. The flush happens at the first swap; after
    // that no deposit or withdraw re-arms it, so the inventory moves must
    // carry the sizes themselves.
    const SIZE_BPS: u64 = 2_000;
    let scale = |atoms: u64, old: u64, new: u64| (atoms as u128 * new as u128 / old as u128) as u64;
    let level = (5_000, SIZE_BPS as u16, u32::MAX);
    let profile = flip_profile(&[level], &[level], 2_000);
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, profile);
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();

    // A hostile depositor goes large, lets the flush size the ladder
    // against its inventory...
    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, 100_000, 0, 400_000, 400_000)
        .expect("outside deposit");
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, 0, 1_000, Price::INFINITY.as_u32(), 1)
        .expect("flush swap");
    let flushed = f.vault(0);
    let ask0 = flushed.remaining.asks[0];
    assert!(
        ask0.flip_size.get() > 0,
        "the flush swap flipped into quote"
    );

    // ...then exits whole. The level must shrink to its fraction of what
    // remains, not stay sized against the inflated leg.
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    f.withdraw(&alice, 0, shares, 0, 0).expect("full exit");
    let after = f.vault(0);
    let ask_after = after.remaining.asks[0];
    assert_eq!(
        ask_after.size.get(),
        scale(
            ask0.size.get(),
            flushed.base_atoms.get(),
            after.base_atoms.get()
        )
    );
    assert!(
        ask_after.size.get() <= SIZE_BPS * after.base_atoms.get() / 10_000,
        "level 0 rests at most its 20% of the remaining base leg"
    );
    assert_eq!(
        ask_after.flip_size.get(),
        scale(
            ask0.flip_size.get(),
            flushed.quote_atoms.get(),
            after.quote_atoms.get()
        ),
        "the flipped quote allowance shrinks with the quote leg"
    );
    let bid_after = after.remaining.bids[0].size.get();
    assert!(bid_after <= SIZE_BPS * after.quote_atoms.get() / 10_000);

    // A leader top-up scales both counters back up with their legs.
    f.deposit_leader(0, 100_000, 0, 100_000, 200_000)
        .expect("leader top-up");
    let topped = f.vault(0);
    let ask_topped = topped.remaining.asks[0];
    assert_eq!(
        ask_topped.size.get(),
        scale(
            ask_after.size.get(),
            after.base_atoms.get(),
            topped.base_atoms.get()
        )
    );
    assert_eq!(
        ask_topped.flip_size.get(),
        scale(
            ask_after.flip_size.get(),
            after.quote_atoms.get(),
            topped.quote_atoms.get()
        )
    );

    // A taker sweeping the asks fills exactly that rescaled size (the
    // bound itself is pinned by the assertions above).
    f.svm.expire_blockhash();
    let meta = f
        .swap_meta(&taker, 0, 900_000, Price::INFINITY.as_u32(), 1)
        .expect("sweep");
    let fills = events::fills(&meta);
    assert_eq!(fills.len(), 1, "one ask level on the book");
    assert_eq!(fills[0].fill_base.get(), ask_topped.size.get());
}

#[test]
fn a_deposit_round_trip_leaves_the_flipped_allowance_intact() {
    // An outside depositor who enters after the flush and exits whole must
    // not erode the leader's flipped proceeds: deposit and withdraw scale
    // `flip_size` the same way, so the round trip is neutral to floor dust.
    let level = (5_000, 2_000, u32::MAX);
    let mut f = Fixture::seeded_with(
        SEED_BASE,
        SEED_QUOTE,
        flip_profile(&[level], &[level], 2_000),
    );
    let admin = f.authority.insecure_clone();
    let leader = f.authority.insecure_clone();
    f.set_outside_deposits_approved(&admin, 0, true).unwrap();
    f.set_allow_outside_depositors(&leader, 0, true).unwrap();
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, 0, 10_000, Price::INFINITY.as_u32(), 1)
        .expect("flush swap flips the ask into quote");
    let before = f.vault(0).remaining.asks[0];
    assert!(before.flip_size.get() > 0);

    let alice = f.funded_depositor(400_000, 400_000);
    f.deposit(&alice, 0, 100_000, 0, 400_000, 400_000)
        .expect("outside deposit");
    let shares = f.vault_depositor(0, &alice.pubkey()).unwrap().shares.get();
    f.withdraw(&alice, 0, shares, 0, 0).expect("full exit");

    let after = f.vault(0).remaining.asks[0];
    for (name, was, now) in [
        ("size", before.size.get(), after.size.get()),
        ("flip_size", before.flip_size.get(), after.flip_size.get()),
    ] {
        assert!(
            was.abs_diff(now) <= 2,
            "{name} moved {was} -> {now} across a deposit round trip"
        );
    }
}
