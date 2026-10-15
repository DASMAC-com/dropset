//! Flip-in-place matching — a fill debits the consumed counter of a level
//! and credits the same level's other counter by exactly the atoms
//! inventory was credited, so the proceeds rest on the opposite side at
//! the level's `flip_price`. End-to-end against the deployed `.so` on the
//! shared [`Fixture`].

mod common;

use anchor_v2_testing::{Keypair, Message, Signer, VersionedMessage, VersionedTransaction};
use common::fixture::{flip_profile, simple_profile, Fixture, PROFILE_BYTES};
use dropset::{FillEvent, Price, Vault, N_LEVELS};
use litesvm::types::TransactionMetadata;
use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

const SEED_BASE: u64 = 1_000_000;
const SEED_QUOTE: u64 = 1_085_000;
const BUY: u8 = 0;
const SELL: u8 = 1;
/// The flip spread every flipping profile here quotes, in ppm.
const SPREAD: u32 = 2_000;
const PPM: u128 = 1_000_000;

/// One ask and one bid, ±0.5% off the reference, each the whole leg.
fn two_sided() -> [u8; PROFILE_BYTES] {
    flip_profile(
        &[(5_000, 10_000, u32::MAX)],
        &[(5_000, 10_000, u32::MAX)],
        SPREAD,
    )
}

fn only_fill(meta: &TransactionMetadata) -> FillEvent {
    let fills = common::events::fills(meta);
    assert_eq!(fills.len(), 1, "expected exactly one leg");
    fills[0]
}

/// Custody holds — `treasury_residual` itself asserts each treasury covers
/// every claim — and the only unattributed slack is exact-in dust, at most
/// a couple of input atoms per taker-bound swap.
fn assert_custody(f: &Fixture, swaps: u64) {
    let (base, quote) = f.treasury_residual();
    assert!(
        base <= 2 * swaps && quote <= 2 * swaps,
        "residual ({base}, {quote}) after {swaps} swaps is more than dust"
    );
}

/// Base resting on the ask side (own asks plus flipped bids) and quote
/// resting on the bid side (own bids plus flipped asks).
fn resting(v: &Vault) -> (u64, u64) {
    let base = (0..N_LEVELS)
        .map(|i| v.remaining.asks[i].size.get() + v.remaining.bids[i].flip_size.get())
        .sum();
    let quote = (0..N_LEVELS)
        .map(|i| v.remaining.bids[i].size.get() + v.remaining.asks[i].flip_size.get())
        .sum();
    (base, quote)
}

#[test]
fn an_ask_fill_flips_and_the_round_trip_grows_the_level_by_the_spread() {
    let profile = flip_profile(&[(5_000, 10_000, u32::MAX)], &[], SPREAD);
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, profile);
    let taker = f.funded_depositor(1_000_000, 1_000_000);

    let fill = only_fill(
        &f.swap_meta(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("buy"),
    );
    let (dx, dq) = (fill.fill_base, fill.fill_quote);
    let lvl = f.vault(0).remaining.asks[0];
    assert_eq!(
        lvl.size.get(),
        SEED_BASE - dx,
        "own side debited the output"
    );
    assert_eq!(
        lvl.flip_size.get(),
        dq,
        "flip credited the priced input leg"
    );
    assert!(lvl.flip_price.is_matchable() && lvl.flip_price < lvl.price);

    // The flipped bid is the only bid on the book; a large sell takes it
    // whole, so the level is bound and no residue applies.
    let back = only_fill(
        &f.swap_meta(&taker, SELL, 1_000_000, Price::ZERO.as_u32(), 1)
            .expect("sell back"),
    );
    assert_eq!((back.sector_idx, back.level_idx), (0, 0));
    assert_eq!(back.fill_price, lvl.flip_price, "fills at the flip price");
    assert_eq!(back.fill_quote, dq, "the whole flipped bid fills");
    let credited = lvl.flip_price.base_for_quote(dq) as u64;
    assert_eq!(back.fill_base, credited);

    let after = f.vault(0).remaining.asks[0];
    assert_eq!(after.flip_size.get(), 0);
    // Pinned exactly: the level regains dx / (1 - r), toward zero.
    assert_eq!(after.size.get(), SEED_BASE - dx + credited);
    let ideal = (dx as u128 * PPM / (PPM - SPREAD as u128)) as u64;
    assert!(
        credited <= ideal && ideal - credited <= 2,
        "regained {credited}, ideal dx / (1 - r) = {ideal}"
    );
    assert!(after.size.get() > SEED_BASE, "grew by the captured spread");
    assert_custody(&f, 2);
}

#[test]
fn a_bid_fill_flips_and_the_round_trip_grows_the_level_by_the_spread() {
    let profile = flip_profile(&[], &[(5_000, 10_000, u32::MAX)], SPREAD);
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, profile);
    let taker = f.funded_depositor(1_000_000, 1_000_000);

    let fill = only_fill(
        &f.swap_meta(&taker, SELL, 100_000, Price::ZERO.as_u32(), 1)
            .expect("sell"),
    );
    let (db, dq) = (fill.fill_base, fill.fill_quote);
    let lvl = f.vault(0).remaining.bids[0];
    assert_eq!(lvl.size.get(), SEED_QUOTE - dq);
    assert_eq!(lvl.flip_size.get(), db);
    assert!(lvl.flip_price.is_matchable() && lvl.flip_price > lvl.price);

    let back = only_fill(
        &f.swap_meta(&taker, BUY, 1_000_000, Price::INFINITY.as_u32(), 1)
            .expect("buy back"),
    );
    assert_eq!(back.fill_price, lvl.flip_price);
    assert_eq!(back.fill_base, db, "the whole flipped ask fills");
    let credited = lvl.flip_price.quote_for_base(db) as u64;
    assert_eq!(back.fill_quote, credited);

    let after = f.vault(0).remaining.bids[0];
    assert_eq!(after.flip_size.get(), 0);
    // Pinned exactly: the level regains dq × (1 + r), toward zero.
    assert_eq!(after.size.get(), SEED_QUOTE - dq + credited);
    let ideal = (dq as u128 * (PPM + SPREAD as u128) / PPM) as u64;
    assert!(
        credited <= ideal && ideal - credited <= 3,
        "regained {credited}, ideal dq × (1 + r) = {ideal}"
    );
    assert!(after.size.get() > SEED_QUOTE);
    assert_custody(&f, 2);
}

#[test]
fn a_partial_fill_quotes_both_sides_of_the_level() {
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, two_sided());
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("partial buy");
    let ask = f.vault(0).remaining.asks[0];
    assert!(ask.size.get() > 0 && ask.flip_size.get() > 0);

    // The flipped bid (just under the ask) outranks the level's own bid
    // (0.5% under the reference), so a small sell lands on it.
    let sell = only_fill(
        &f.swap_meta(&taker, SELL, 10_000, Price::ZERO.as_u32(), 1)
            .expect("sell into the flip"),
    );
    assert_eq!(sell.level_idx, 0);
    assert_eq!(sell.fill_price, ask.flip_price);
    let ask = f.vault(0).remaining.asks[0];
    assert!(ask.size.get() > 0 && ask.flip_size.get() > 0, "still both");

    // And the same level's own ask still fills a buy.
    let buy = only_fill(
        &f.swap_meta(&taker, BUY, 10_000, Price::INFINITY.as_u32(), 1)
            .expect("buy the own ask"),
    );
    assert_eq!(buy.level_idx, 0);
    assert_eq!(buy.fill_price, ask.price);
    assert_custody(&f, 3);
}

#[test]
fn a_whipsaw_conserves_inventory_and_bounds_drainage() {
    let levels = [(5_000, 4_000, u32::MAX), (10_000, 4_000, u32::MAX)];
    let mut f = Fixture::seeded_with(
        SEED_BASE,
        SEED_QUOTE,
        flip_profile(&levels, &levels, SPREAD),
    );
    let taker = f.funded_depositor(5_000_000, 5_000_000);
    // The flush allowance per direction: 80% of each leg.
    let (base_floor, quote_floor) = (SEED_BASE / 5, SEED_QUOTE / 5);

    let mut slack: Option<(u64, u64)> = None;
    for round in 0..8 {
        let (side, limit) = if round % 2 == 0 {
            (BUY, Price::INFINITY)
        } else {
            (SELL, Price::ZERO)
        };
        f.swap(&taker, side, 300_000 + round * 1_000, limit.as_u32(), 1)
            .expect("whipsaw leg");
        let v = f.vault(0);
        let (base, quote) = (v.base_atoms.get(), v.quote_atoms.get());
        let (rest_base, rest_quote) = resting(&v);
        assert!(
            rest_base <= base && rest_quote <= quote,
            "allowance <= inventory"
        );
        // Equal atoms move on both ledgers, so the unquoted remainder of
        // each leg never moves once the flush has set it.
        let now = (base - rest_base, quote - rest_quote);
        assert_eq!(*slack.get_or_insert(now), now, "round {round}");
        assert!(base >= base_floor && quote >= quote_floor, "drainage bound");
        assert_custody(&f, round + 1);
    }
}

#[test]
fn a_flush_resets_flipped_state() {
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, two_sided());
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("buy");
    assert!(f.vault(0).remaining.asks[0].flip_size.get() > 0);

    let auth = f.authority.insecure_clone();
    let fresh = Price::encode(10_860_000, 0).unwrap();
    f.set_reference_price(&auth, 0, fresh.as_u32(), 1)
        .expect("re-arm the flush");
    let fill = only_fill(
        &f.swap_meta(&taker, SELL, 10_000, Price::ZERO.as_u32(), 1)
            .expect("sell after the flush"),
    );

    // The ask's flip is gone, so the sell met the level's own bid instead.
    let v = f.vault(0);
    assert_eq!(v.remaining.asks[0].flip_size.get(), 0, "flush re-centered");
    assert_eq!(fill.fill_price, v.remaining.bids[0].price);
    assert_eq!(v.remaining.bids[0].flip_size.get(), fill.fill_base);
}

#[test]
fn expiry_governs_both_counters() {
    let levels = [(5_000, 10_000, 60)];
    let mut f = Fixture::seeded_with(
        SEED_BASE,
        SEED_QUOTE,
        flip_profile(&levels, &levels, SPREAD),
    );
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("buy while live");
    let before = f.vault(0).remaining.asks[0];
    assert!(before.size.get() > 0 && before.flip_size.get() > 0);

    f.warp_unix(120);
    for (side, limit) in [(SELL, Price::ZERO), (BUY, Price::INFINITY)] {
        let meta = f
            .swap_meta(&taker, side, 10_000, limit.as_u32(), 0)
            .expect("no-fill swap returns Ok");
        assert!(
            common::events::fills(&meta).is_empty(),
            "expired level filled"
        );
    }
    let after = f.vault(0).remaining.asks[0];
    assert_eq!(after.size.get(), before.size.get());
    assert_eq!(after.flip_size.get(), before.flip_size.get());
}

#[test]
fn a_zero_spread_never_flips() {
    let mut f = Fixture::seeded_with(
        SEED_BASE,
        SEED_QUOTE,
        simple_profile(5_000, 10_000, u32::MAX),
    );
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("buy");
    // Without a flipped bid above it, the sell meets the level's own bid.
    let sell = only_fill(
        &f.swap_meta(&taker, SELL, 10_000, Price::ZERO.as_u32(), 1)
            .expect("sell"),
    );
    let v = f.vault(0);
    assert_eq!(sell.fill_price, v.remaining.bids[0].price);
    for p in v.remaining.asks.iter().chain(v.remaining.bids.iter()) {
        assert_eq!(p.flip_price, Price::ZERO, "zero spread writes the sentinel");
        assert_eq!(p.flip_size.get(), 0, "zero spread never credits a flip");
    }
}

#[test]
fn a_soft_revert_restores_both_counters() {
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, two_sided());
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    f.swap(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
        .expect("buy");
    let before = f.vault(0);

    // One revert consuming the flip (credits `size`), one consuming the
    // own ask (credits `flip_size`): each must restore both counters.
    for (side, limit) in [(SELL, Price::ZERO), (BUY, Price::INFINITY)] {
        f.swap(&taker, side, 50_000, limit.as_u32(), u64::MAX)
            .expect("soft revert");
        let after = f.vault(0);
        for i in 0..N_LEVELS {
            for (a, b) in [
                (after.remaining.asks[i], before.remaining.asks[i]),
                (after.remaining.bids[i], before.remaining.bids[i]),
            ] {
                assert_eq!(a.size.get(), b.size.get(), "size restored");
                assert_eq!(a.flip_size.get(), b.flip_size.get(), "flip restored");
            }
        }
        assert_eq!(after.base_atoms.get(), before.base_atoms.get());
        assert_eq!(after.quote_atoms.get(), before.quote_atoms.get());
    }
}

// ── The full-flip compute measurement ────────────────────────────────

const COMPUTE_BUDGET_ID: Pubkey =
    Pubkey::from_str_const("ComputeBudget111111111111111111111111111111");
/// The per-transaction ceiling a `SetComputeUnitLimit` can request.
const MAX_TX_CU: u32 = 1_400_000;
/// The compute-budget program's `SetComputeUnitLimit` discriminant.
const SET_COMPUTE_UNIT_LIMIT: u8 = 2;
const VAULTS: u32 = 10;
/// Floor on the measured take — one tie group of ten legs, the deepest
/// step that fits today — so a change that eats into the heap headroom
/// fails here rather than on the first deep sweep in production.
const MIN_LEGS: usize = 10;

/// Send `ix` behind a `SetComputeUnitLimit` at the transaction ceiling,
/// so the measurement reports what the take costs rather than where the
/// 200k default cuts it off. `Err` carries the runtime error and the log
/// line that names it.
fn send_at_max_cu(
    f: &mut Fixture,
    payer: &Keypair,
    ix: Instruction,
) -> Result<TransactionMetadata, String> {
    let mut data = vec![SET_COMPUTE_UNIT_LIMIT];
    data.extend_from_slice(&MAX_TX_CU.to_le_bytes());
    let limit = Instruction::new_with_bytes(COMPUTE_BUDGET_ID, &data, vec![]);
    let blockhash = f.svm.latest_blockhash();
    let msg = Message::new_with_blockhash(&[limit, ix], Some(&payer.pubkey()), &blockhash);
    let txn = VersionedTransaction::try_new(VersionedMessage::Legacy(msg), &[payer]).unwrap();
    f.svm.send_transaction(txn).map_err(|e| {
        let last = e.meta.logs.last().cloned().unwrap_or_default();
        format!("{:?}: {last}", e.err)
    })
}

/// The take a fully flipped ten-vault book allows — the figure the
/// `N_LEVELS` sizing question waits on.
///
/// Every bid level of every vault is sold into first, so each has flipped
/// to a resting ask: a Buy then collects one own and one flipped entry per
/// level, `2 × N_LEVELS × VAULTS`, the most a single take can see. The
/// sells go one level at a time (a limit at that level's price), ten legs
/// each, because what bounds a take is **not** compute: every filled leg
/// buffers a `FillEvent` (208 B) for `lib.rs` to emit, in the 32 KiB bump
/// heap that never reclaims, so the heap runs out long before the
/// transaction's 1.4M CU do. The measured Buy therefore searches down from
/// the whole book for the largest limit price whose take still completes.
#[test]
fn a_fully_flipped_ten_vault_book_measures_the_largest_take() {
    // Every level of every vault quotes a spread, 12% of each leg a level.
    let ladder: Vec<(u32, u16, u32)> = (0..N_LEVELS as u32)
        .map(|i| ((i + 1) * 1_000, 1_200, u32::MAX))
        .collect();
    let profile = flip_profile(&ladder, &ladder, 500);
    let mut f = Fixture::bootstrap();
    let auth = f.authority.insecure_clone();
    let reference = Price::encode(10_850_000, 0).unwrap().as_u32();
    for i in 0..VAULTS {
        let leader = f.funded_keypair(10 * common::SIGNER_FUNDING_LAMPORTS);
        f.create_vault(0, auth.pubkey(), false, leader.pubkey())
            .expect("vault");
        f.set_reference_price(&auth, i, reference, 0).expect("ref");
        f.set_liquidity_profile(&auth, i, profile).expect("profile");
        f.deposit_leader_as(&leader, i, SEED_BASE, SEED_QUOTE, SEED_BASE, SEED_QUOTE)
            .expect("seed");
        f.svm.expire_blockhash();
    }
    let taker = f.funded_depositor(20_000_000, 50_000_000);

    // A reverted sell materializes every vault's ladder without filling,
    // so the per-level limit prices can be read off the book. It is also
    // the cold-book cost: every vault flushed, nothing filled.
    let prime = f.swap_ix(&taker.pubkey(), SELL, 1, Price::ZERO.as_u32(), u64::MAX);
    let flush_cu = send_at_max_cu(&mut f, &taker, prime)
        .expect("priming revert")
        .compute_units_consumed;
    for level in 0..N_LEVELS {
        let limit = f.vault(0).remaining.bids[level].price.as_u32();
        let sell = f.swap_ix(&taker.pubkey(), SELL, 20_000_000, limit, 1);
        let meta = send_at_max_cu(&mut f, &taker, sell).expect("flip one level");
        assert_eq!(common::events::fills(&meta).len(), VAULTS as usize);
    }
    let mut prices = buy_prices(&f);
    let entries = prices.len();
    assert_eq!(entries, 2 * N_LEVELS * VAULTS as usize, "every bid flipped");

    // Distinct limit prices from the far end of the book inward; ties
    // across the identical vaults move ten legs at a time.
    prices.dedup();
    let (mut legs, mut cu, mut stopped_by) = (0, 0, String::new());
    for &limit in prices.iter().rev() {
        let buy = f.swap_ix(&taker.pubkey(), BUY, 50_000_000, limit, 1);
        match send_at_max_cu(&mut f, &taker, buy) {
            Ok(meta) => {
                legs = common::events::fills(&meta).len();
                cu = meta.compute_units_consumed;
                break;
            }
            Err(e) if stopped_by.is_empty() => stopped_by = e,
            Err(_) => {}
        }
    }
    println!(
        "full-flip take over {VAULTS} vaults: {entries} entries collected, at most \
         {legs} legs filled in {cu} CU of the {MAX_TX_CU} transaction budget; the \
         whole book failed on: {stopped_by}. Flushing all {VAULTS} vaults with no \
         fill: {flush_cu} CU"
    );
    assert!(
        legs >= MIN_LEGS,
        "a take fills at least {MIN_LEGS} legs, got {legs}"
    );
    // What stops the whole-book take is the heap, as the doc above claims —
    // not compute, and not a fault in the flipped path.
    assert!(
        stopped_by.contains("heap"),
        "the whole-book take should fail on the heap, failed on: {stopped_by}"
    );
    assert_custody(&f, N_LEVELS as u64 + 1);
}

/// Every price a Buy would meet on the book, ascending: each vault's own
/// asks and each bid level's flipped ask.
fn buy_prices(f: &Fixture) -> Vec<u32> {
    let mut prices = Vec::new();
    for i in 0..VAULTS {
        let v = f.vault(i);
        for l in 0..N_LEVELS {
            if v.remaining.asks[l].size.get() > 0 {
                prices.push(v.remaining.asks[l].price.as_u32());
            }
            if v.remaining.bids[l].flip_size.get() > 0 {
                prices.push(v.remaining.bids[l].flip_price.as_u32());
            }
        }
    }
    prices.sort_unstable();
    prices
}
