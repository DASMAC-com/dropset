//! Flip-in-place matching — a fill debits the consumed counter of a level
//! and credits the same level's other counter by exactly the atoms
//! inventory was credited, so the proceeds rest on the opposite side at
//! the level's `flip_price`. End-to-end against the deployed `.so` on the
//! shared [`Fixture`].

mod common;

use anchor_v2_testing::{Keypair, Message, Signer, VersionedMessage, VersionedTransaction};
use common::fixture::{flip_profile, simple_profile, Fixture, PROFILE_BYTES};
use dropset::{FillEvent, Price, Vault, LEG_KIND_FLIPPED, LEG_KIND_PRIMARY, N_LEVELS};
use dropset_sdk::SWAP_COMPUTE_UNIT_LIMIT;
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
    let (dx, dq) = (fill.fill_base.get(), fill.fill_quote.get());
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
    // A primary leg: it consumed `size`, reports what is left of it, and
    // carries the level's flip price so the spread reads off the event.
    assert_eq!(fill.leg_kind, LEG_KIND_PRIMARY);
    assert_eq!(fill.remaining_after.get(), lvl.size.get());
    assert_eq!(fill.flip_price, lvl.flip_price);

    // The flipped bid is the only bid on the book; a large sell takes it
    // whole, so the level is bound and no residue applies.
    let back = only_fill(
        &f.swap_meta(&taker, SELL, 1_000_000, Price::ZERO.as_u32(), 1)
            .expect("sell back"),
    );
    assert_eq!((back.sector_idx.get(), back.level_idx.get()), (0, 0));
    assert_eq!(back.fill_price, lvl.flip_price, "fills at the flip price");
    assert_eq!(back.fill_quote.get(), dq, "the whole flipped bid fills");
    let credited = lvl.flip_price.base_for_quote(dq) as u64;
    assert_eq!(back.fill_base.get(), credited);
    // A flipped leg: it consumed `flip_size`, here to zero.
    assert_eq!(back.leg_kind, LEG_KIND_FLIPPED);
    assert_eq!(back.remaining_after.get(), 0);
    assert_eq!(back.flip_price, lvl.flip_price);

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
    let (db, dq) = (fill.fill_base.get(), fill.fill_quote.get());
    let lvl = f.vault(0).remaining.bids[0];
    assert_eq!(lvl.size.get(), SEED_QUOTE - dq);
    assert_eq!(lvl.flip_size.get(), db);
    assert!(lvl.flip_price.is_matchable() && lvl.flip_price > lvl.price);
    // `remaining_after` is in the consumed side's atoms: quote on a bid.
    assert_eq!(fill.leg_kind, LEG_KIND_PRIMARY);
    assert_eq!(fill.remaining_after.get(), SEED_QUOTE - dq);
    assert_eq!(fill.flip_price, lvl.flip_price);

    let back = only_fill(
        &f.swap_meta(&taker, BUY, 1_000_000, Price::INFINITY.as_u32(), 1)
            .expect("buy back"),
    );
    assert_eq!(back.fill_price, lvl.flip_price);
    assert_eq!(back.fill_base.get(), db, "the whole flipped ask fills");
    let credited = lvl.flip_price.quote_for_base(db) as u64;
    assert_eq!(back.fill_quote.get(), credited);
    assert_eq!(back.leg_kind, LEG_KIND_FLIPPED);
    assert_eq!(back.remaining_after.get(), 0);
    assert_eq!(back.flip_price, lvl.flip_price);

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
    assert_eq!(sell.level_idx.get(), 0);
    assert_eq!(sell.fill_price, ask.flip_price);
    let ask = f.vault(0).remaining.asks[0];
    assert!(ask.size.get() > 0 && ask.flip_size.get() > 0, "still both");
    // A flipped leg that leaves the counter non-zero reports what is left
    // of `flip_size`, not of `size` and not a drained zero.
    assert_eq!(sell.leg_kind, LEG_KIND_FLIPPED);
    assert_eq!(sell.remaining_after.get(), ask.flip_size.get());
    assert_eq!(sell.flip_price, ask.flip_price);

    // And the same level's own ask still fills a buy.
    let buy = only_fill(
        &f.swap_meta(&taker, BUY, 10_000, Price::INFINITY.as_u32(), 1)
            .expect("buy the own ask"),
    );
    assert_eq!(buy.level_idx.get(), 0);
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
    assert_eq!(v.remaining.bids[0].flip_size.get(), fill.fill_base.get());
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
    assert_eq!(sell.flip_price, Price::ZERO, "the event reports no flip");
}

#[test]
fn a_full_spread_flushes_to_no_flip_and_reports_zero() {
    // A 100% spread puts the ask's flip bid at zero, so the flush stores
    // the `Price::ZERO` sentinel rather than a price, even though the
    // spread itself is non-zero. The level never flips, and the event
    // carries that zero through.
    let profile = flip_profile(&[(5_000, 10_000, u32::MAX)], &[], 1_000_000);
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, profile);
    let taker = f.funded_depositor(1_000_000, 1_000_000);
    let fill = only_fill(
        &f.swap_meta(&taker, BUY, 100_000, Price::INFINITY.as_u32(), 1)
            .expect("buy"),
    );
    let lvl = f.vault(0).remaining.asks[0];
    assert!(!lvl.flip_price.is_matchable(), "the flip cannot be matched");
    assert_eq!(lvl.flip_size.get(), 0, "so nothing was credited");
    assert_eq!(fill.leg_kind, LEG_KIND_PRIMARY);
    assert_eq!(fill.flip_price, Price::ZERO, "the event reports no flip");
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
/// buffers a `FillEvent` (256 B) for `lib.rs` to emit, in the 32 KiB bump
/// heap that never reclaims, so the heap runs out long before the
/// transaction's 1.4M CU do. The measured Buy therefore searches down from
/// the whole book for the largest limit price whose take still completes.
#[test]
fn a_fully_flipped_ten_vault_book_measures_the_largest_take() {
    let (mut f, taker) = ten_vault_book();

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

/// The most legs one take completes on the ten-vault book: a seventeenth
/// aborts on the 32 KiB heap, since each filled leg buffers a `FillEvent`
/// the bump allocator never reclaims. The SDK's `compute_budget` docs cite
/// this figure.
const HEAP_CEILING_LEGS: usize = 16;

/// The costliest take that fills without stranding dust: a take on a cold
/// ten-vault book (every vault flushed in the same transaction) sized leg
/// by leg to the heap ceiling, on each side. It must fit what a bare `swap`
/// gets with no budget instruction.
#[test]
fn a_cold_take_at_the_heap_ceiling_fits_the_default_instruction_budget() {
    for side in [SELL, BUY] {
        let (legs, cu, stopped_by) = cold_take_to_the_heap_ceiling(side);
        println!(
            "cold side-{side} take to the heap ceiling over {VAULTS} vaults: {legs} \
             legs, costliest {cu} CU; the next leg failed on: {stopped_by}"
        );
        assert_eq!(
            legs, HEAP_CEILING_LEGS,
            "side {side}: legs a take completes"
        );
        assert!(
            stopped_by.contains("heap"),
            "side {side}: the next leg should stop on the heap, stopped on: {stopped_by}"
        );
        assert!(
            cu < common::DEFAULT_INSTRUCTION_CU,
            "side {side}: the costliest cold take used {cu} CU, over the {} a bare \
             swap gets",
            common::DEFAULT_INSTRUCTION_CU
        );
    }
}

/// Size `side` takes on fresh cold books one leg at a time, from one full
/// tie group up, until one fails. Returns the most legs a take completed,
/// the costliest completing take's CU, and the failure that stopped it.
///
/// Legs move ten at a time across the identical vaults when sized by limit
/// price, so the sizes are built from input atoms instead: what the first
/// tie group consumes, plus whole legs of the second, both read off a
/// primed twin (the build is deterministic, and priming the measured book
/// would warm it). Each size also runs one atom over, the remainder a
/// level-capped leg can leave behind.
fn cold_take_to_the_heap_ceiling(side: u8) -> (usize, u64, String) {
    let (dust_limit, budget) = if side == SELL {
        (Price::ZERO.as_u32(), 20_000_000)
    } else {
        (Price::INFINITY.as_u32(), 50_000_000)
    };
    let (mut twin, primer) = ten_vault_book();
    let prime = twin.swap_ix(&primer.pubkey(), side, 1, dust_limit, u64::MAX);
    send_at_max_cu(&mut twin, &primer, prime).expect("priming revert");
    let levels = {
        let v = twin.vault(0);
        let book = if side == SELL {
            &v.remaining.bids
        } else {
            &v.remaining.asks
        };
        [book[0].price.as_u32(), book[1].price.as_u32()]
    };
    let ata = if side == SELL {
        twin.base_ata(&primer.pubkey())
    } else {
        twin.quote_ata(&primer.pubkey())
    };
    let mut consumed = Vec::new();
    for limit in levels {
        let before = twin.token_balance(&ata);
        twin.svm.expire_blockhash();
        let take = twin.swap_ix(&primer.pubkey(), side, budget, limit, 1);
        let meta = send_at_max_cu(&mut twin, &primer, take).expect("one tie group");
        assert_eq!(common::events::fills(&meta).len(), VAULTS as usize);
        consumed.push(before - twin.token_balance(&ata));
    }
    let (first_group, per_leg) = (consumed[0], consumed[1] / u64::from(VAULTS));

    let (mut legs, mut cu) = (0, 0);
    for extra in 0..u64::from(VAULTS) {
        for remainder in [0, 1] {
            let (mut f, taker) = ten_vault_book();
            let amount = first_group + extra * per_leg + remainder;
            let take = f.swap_ix(&taker.pubkey(), side, amount, dust_limit, 1);
            match send_at_max_cu(&mut f, &taker, take) {
                Ok(meta) => {
                    legs = legs.max(common::events::fills(&meta).len());
                    cu = cu.max(meta.compute_units_consumed);
                }
                Err(stopped_by) => return (legs, cu, stopped_by),
            }
        }
    }
    (legs, cu, String::new())
}

/// The costliest take that completes: a one-atom sell on a fully flipped
/// ten-vault book, the most entries a sell can see (every vault's own bids
/// plus every ask flipped to a bid).
///
/// One base atom buys a quote atom at any bid above 1, but that quote atom
/// buys back no whole base atom, so each leg is skipped rather than ending
/// the walk and the take visits every entry without filling. It overruns
/// what a bare `swap` gets, so this is the case [`SWAP_COMPUTE_UNIT_LIMIT`]
/// is sized from.
#[test]
fn a_dust_sell_walking_a_flipped_book_fits_the_client_compute_limit() {
    let (mut f, taker) = ten_vault_book();
    let prime = f.swap_ix(&taker.pubkey(), BUY, 1, Price::INFINITY.as_u32(), u64::MAX);
    send_at_max_cu(&mut f, &taker, prime).expect("priming revert");
    for level in 0..N_LEVELS {
        let limit = f.vault(0).remaining.asks[level].price.as_u32();
        let buy = f.swap_ix(&taker.pubkey(), BUY, 50_000_000, limit, 1);
        let meta = send_at_max_cu(&mut f, &taker, buy).expect("flip one level");
        assert_eq!(common::events::fills(&meta).len(), VAULTS as usize);
    }
    let entries: usize = (0..VAULTS)
        .map(|i| {
            let v = f.vault(i);
            (0..N_LEVELS)
                .map(|l| {
                    usize::from(v.remaining.bids[l].size.get() > 0)
                        + usize::from(v.remaining.asks[l].flip_size.get() > 0)
                })
                .sum::<usize>()
        })
        .sum();
    assert_eq!(entries, 2 * N_LEVELS * VAULTS as usize, "every ask flipped");

    let dust = f.swap_ix(&taker.pubkey(), SELL, 1, Price::ZERO.as_u32(), 0);
    let meta = send_at_max_cu(&mut f, &taker, dust).expect("dust sell");
    let cu = meta.compute_units_consumed;
    println!("dust sell over {entries} flipped-book entries: {cu} CU, nothing filled");
    assert!(
        common::events::fills(&meta).is_empty(),
        "a dust sell fills nothing"
    );
    assert!(
        cu < u64::from(SWAP_COMPUTE_UNIT_LIMIT),
        "the full-book dust walk used {cu} CU, over the {SWAP_COMPUTE_UNIT_LIMIT} \
         a client sends ahead of a bare swap"
    );
}

/// Ten vaults on one market, every level of every vault quoting a spread
/// at 12% of each leg a level, plus a taker funded to sweep it. Cold:
/// nothing has touched the book since the profiles were set.
fn ten_vault_book() -> (Fixture, Keypair) {
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
    (f, taker)
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
