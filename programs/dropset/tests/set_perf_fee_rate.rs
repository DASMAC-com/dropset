//! `set_perf_fee_rate` integration tests — the leader's performance-fee
//! retune. Covers realize-first (an excess accrued under the old rate mints
//! at the old rate, never the new), the monotone high-water mark across a
//! change in either direction, the 100% bound, the frozen and tombstoned
//! no-op realize, the event, and the authority / same-rate / sector gates.

mod common;

use anchor_lang::Discriminator;
use anchor_v2_testing::Signer;
use common::{
    events,
    fixture::{simple_profile, Fixture},
};
use dropset::{Price, RealizeEvent, Q32_32_ONE};
use dropset_math_core::share::realize_perf_fee;
use solana_keypair::Keypair;

/// 10% of gains, in ppm.
const TEN_PCT: u32 = 100_000;
/// 20% of gains, in ppm.
const TWENTY_PCT: u32 = 200_000;
/// The program's 100% bound, in ppm.
const PPM: u32 = 1_000_000;

/// Bootstrap → a vault at `rate` led by a distinct funded leader → quoted
/// and seeded. Returns the leader.
fn seeded_at(rate: u32) -> (Fixture, Keypair) {
    let mut f = Fixture::bootstrap();
    let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
    f.create_vault(rate, leader.pubkey(), false, leader.pubkey())
        .expect("create_vault");
    let px = Price::encode(10_850_000, 0).unwrap();
    f.set_reference_price(&leader, 0, px.as_u32(), 0)
        .expect("set_reference_price");
    f.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
        .expect("set_liquidity_profile");
    f.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
        .expect("seed");
    (f, leader)
}

/// What `realize_in_place` would do to vault 0 at `rate`, from its current
/// on-chain state.
fn kernel_at(f: &Fixture, rate: u32) -> dropset_math_core::share::RealizeResult {
    let v = f.vault(0);
    realize_perf_fee(
        v.base_atoms.get(),
        v.quote_atoms.get(),
        v.total_shares.get(),
        v.leader_shares.get(),
        v.hwm.get(),
        rate,
    )
}

#[test]
fn an_excess_accrued_under_the_old_rate_mints_at_the_old_rate() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    // Drop the mark well below value-per-share: an excess accrued under 10%
    // that no flow has realized yet.
    f.poke_hwm(0, Q32_32_ONE / 2);
    let at_old = kernel_at(&f, TEN_PCT);
    let at_new = kernel_at(&f, TWENTY_PCT);
    assert!(
        at_old.shares_minted > 0,
        "the excess is large enough to fee"
    );
    assert!(
        at_new.shares_minted > at_old.shares_minted,
        "the two rates mint distinguishably"
    );

    let meta = f
        .set_perf_fee_rate_meta(&leader, 0, TWENTY_PCT)
        .expect("leader may raise the fee");

    let re = events::realize(&meta);
    assert_eq!(re.shares_minted, at_old.shares_minted, "minted at 10%");
    let v = f.vault(0);
    assert_eq!(v.leader_shares.get(), at_old.leader_shares_after);
    assert_eq!(v.total_shares.get(), at_old.total_shares_after);
    assert_eq!(v.hwm.get(), at_old.hwm_after);
    assert_eq!(v.perf_fee_rate.get(), TWENTY_PCT);

    // The realize came first in the instruction, then the retune record.
    let payloads = events::event_payloads(&meta);
    assert_eq!(payloads.len(), 2);
    assert!(payloads[0].starts_with(RealizeEvent::DISCRIMINATOR));
}

#[test]
fn the_mark_stays_monotone_across_a_change_in_either_direction() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    f.poke_hwm(0, Q32_32_ONE / 2);

    // Up: the realize lifts the mark to value-per-share.
    f.set_perf_fee_rate(&leader, 0, TWENTY_PCT).expect("raise");
    let after_raise = f.vault(0).hwm.get();
    assert!(after_raise > Q32_32_ONE / 2);

    // Down: nothing new to realize, so neither a mint nor a move.
    f.svm.expire_blockhash();
    let shares = f.vault(0).total_shares.get();
    let meta = f
        .set_perf_fee_rate_meta(&leader, 0, TEN_PCT / 2)
        .expect("lower");
    assert_eq!(events::count::<RealizeEvent>(&meta), 0);
    assert_eq!(f.vault(0).hwm.get(), after_raise);
    assert_eq!(f.vault(0).total_shares.get(), shares);

    // A mark above value-per-share is never pulled down by a change.
    let high = after_raise * 2;
    f.poke_hwm(0, high);
    f.svm.expire_blockhash();
    f.set_perf_fee_rate(&leader, 0, TWENTY_PCT)
        .expect("raise again");
    assert_eq!(f.vault(0).hwm.get(), high);
}

#[test]
fn the_rate_is_bounded_at_one_hundred_percent() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    let err = f
        .set_perf_fee_rate(&leader, 0, PPM + 1)
        .expect_err("above 100% must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidPerfFeeRate);
    assert_eq!(f.vault(0).perf_fee_rate.get(), TEN_PCT);

    f.set_perf_fee_rate(&leader, 0, PPM)
        .expect("exactly 100% is in range");
    assert_eq!(f.vault(0).perf_fee_rate.get(), PPM);
    // And zero is a rate too.
    f.set_perf_fee_rate(&leader, 0, 0).expect("0% is in range");
    assert_eq!(f.vault(0).perf_fee_rate.get(), 0);
}

#[test]
fn a_frozen_vault_realizes_nothing_but_takes_the_new_rate() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    f.poke_hwm(0, Q32_32_ONE / 2);
    let admin = f.authority.insecure_clone();
    f.freeze_vault(&admin, 0).expect("admin freezes vault");
    let (shares, hwm) = (f.vault(0).total_shares.get(), f.vault(0).hwm.get());

    let meta = f
        .set_perf_fee_rate_meta(&leader, 0, TWENTY_PCT)
        .expect("the retune is allowed on a frozen vault");

    assert_eq!(events::count::<RealizeEvent>(&meta), 0, "no fee accrues");
    assert_eq!(f.vault(0).total_shares.get(), shares);
    assert_eq!(f.vault(0).hwm.get(), hwm, "the mark stays pinned");
    assert_eq!(f.vault(0).perf_fee_rate.get(), TWENTY_PCT);
}

#[test]
fn a_tombstoned_vault_realizes_nothing_but_takes_the_new_rate() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    f.poke_hwm(0, Q32_32_ONE / 2);
    f.close_vault(&leader, 0).expect("leader closes vault");
    let (shares, hwm) = (f.vault(0).total_shares.get(), f.vault(0).hwm.get());

    let meta = f
        .set_perf_fee_rate_meta(&leader, 0, TWENTY_PCT)
        .expect("the retune is allowed on a tombstoned vault");

    assert_eq!(events::count::<RealizeEvent>(&meta), 0, "no fee accrues");
    assert_eq!(f.vault(0).total_shares.get(), shares);
    assert_eq!(f.vault(0).hwm.get(), hwm, "the mark stays pinned");
    assert_eq!(f.vault(0).perf_fee_rate.get(), TWENTY_PCT);
}

#[test]
fn rejects_the_quote_authority_delegate() {
    // The fee is a custody-side term: delegating quoting does not delegate it.
    let (mut f, leader) = seeded_at(TEN_PCT);
    let delegate = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    f.set_quote_authority(&leader, 0, delegate.pubkey())
        .expect("delegate quoting");
    let err = f
        .set_perf_fee_rate(&delegate, 0, TWENTY_PCT)
        .expect_err("the quoting delegate must not retune the fee");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(f.vault(0).perf_fee_rate.get(), TEN_PCT);
}

#[test]
fn the_retune_emits_old_and_new_rate() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    let meta = f
        .set_perf_fee_rate_meta(&leader, 0, TWENTY_PCT)
        .expect("retune");
    let ev = events::set_perf_fee_rate(&meta);
    assert_eq!(ev.market, f.market.to_bytes());
    assert_eq!(ev.sector_idx, 0);
    assert_eq!(ev.vault_seq, f.vault(0).seq.get());
    assert_eq!(ev.leader, leader.pubkey().to_bytes());
    assert_eq!(ev.old_rate, TEN_PCT);
    assert_eq!(ev.new_rate, TWENTY_PCT);
}

#[test]
fn rejects_the_current_rate() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    let err = f
        .set_perf_fee_rate(&leader, 0, TEN_PCT)
        .expect_err("re-stamping the current rate must reject");
    common::assert_program_error(&err, dropset::DropsetError::SamePerfFeeRate);
}

#[test]
fn rejects_non_leader() {
    let (mut f, _leader) = seeded_at(TEN_PCT);
    // The admin is not this vault's leader: the gate is the leader alone.
    let admin = f.authority.insecure_clone();
    let err = f
        .set_perf_fee_rate(&admin, 0, TWENTY_PCT)
        .expect_err("non-leader must not retune the fee");
    common::assert_program_error(&err, dropset::DropsetError::Unauthorized);
    assert_eq!(f.vault(0).perf_fee_rate.get(), TEN_PCT);
}

#[test]
fn rejects_invalid_idx_and_empty_sector() {
    let (mut f, leader) = seeded_at(TEN_PCT);
    let err = f
        .set_perf_fee_rate(&leader, 99, TWENTY_PCT)
        .expect_err("out-of-range vault_idx must reject");
    common::assert_program_error(&err, dropset::DropsetError::InvalidSectorIndex);

    // The VaultEmpty guard fires before the leader-authorization check.
    f.poke_leader_empty(0);
    let err = f
        .set_perf_fee_rate(&leader, 0, TWENTY_PCT)
        .expect_err("an empty sector must reject");
    common::assert_program_error(&err, dropset::DropsetError::VaultEmpty);
}
