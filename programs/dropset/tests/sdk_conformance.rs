//! Engine ⇄ SDK conformance.
//!
//! The off-chain SDK (`dropset-sdk`) hand-mirrors the on-chain account
//! layout and the `swap` matcher. These tests prove that mirror is
//! faithful: each stands up a real market in litesvm, decodes the *live*
//! account bytes with the SDK's `MarketView`, predicts a fill with the
//! SDK's `simulate_swap`, then runs the **real** `swap` instruction and
//! asserts the SDK's prediction equals the on-chain realized amounts.
//!
//! If the SDK's `Price` math, flush materialization, or layout ever drift
//! from the program, these fail — the guarantee behind "the SDK does what
//! the engine does".
//!
//! `matching.rs`'s original coverage was a single seeded ladder with two
//! single-leg fills, which left the broader matching paths unpinned. This
//! file replays the matcher across a scenario set that exercises the paths
//! most likely to drift — cross-level fills, cross-*vault* price-time
//! priority, input capped at book depth, a limit price that stops mid-book,
//! a non-zero taker fee, and each expiry domain independently — through
//! both the SDK simulator and a litesvm swap.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{dual_expiry_profile, ladder_profile, Fixture};

use dropset_sdk::clock::{SlotTime, WallTime};
use dropset_sdk::layout::MarketView;
use dropset_sdk::matching::{simulate_swap, Quote, SwapSide};
use dropset_sdk::price::Price;
use solana_pubkey::Pubkey;

/// SDK-decoded snapshot of the live market account.
fn market_bytes(f: &Fixture) -> Vec<u8> {
    f.svm.get_account(&f.market).expect("market account").data
}

/// Predict a swap with the SDK against the pre-swap snapshot (exactly as a
/// router would), execute the real on-chain swap, and assert the SDK's
/// prediction equals the realized taker deltas. Returns the SDK [`Quote`]
/// for scenario-specific assertions.
///
/// `taker` must already hold enough input-leg atoms (quote for a Buy, base
/// for a Sell). Most seeded ladders never expire, so the clocks only have to
/// sit at or past the reference datum; the `sdk_expiry_*_domain` cases are
/// the exception — they quote a bound that is finite in one domain and warp
/// that domain's clock past it, which is how the dual gate gets compared
/// against the engine's rather than only against its own vectors.
fn predict_and_execute(
    f: &mut Fixture,
    taker: &Keypair,
    side: SwapSide,
    amount_in: u64,
    limit_price: Price,
) -> Quote {
    predict_and_execute_with_fee(f, taker, side, amount_in, limit_price, None, 0)
}

/// [`predict_and_execute`] with a caller-declared platform fee, so the
/// engine-vs-simulator comparison also covers fee composition.
///
/// This is the **only** harness that pins the two implementations against each
/// other; the conformance vectors pin native Rust against WASM, which is the
/// same `simulate_swap` on both sides and so cannot catch a divergence from
/// the engine. Running it exclusively at `platform_fee_bps = 0` would leave
/// the composition order (`fill → taker fee → platform fee`, each truncating)
/// asserted only within each implementation separately.
#[allow(clippy::too_many_arguments)]
fn predict_and_execute_with_fee(
    f: &mut Fixture,
    taker: &Keypair,
    side: SwapSide,
    amount_in: u64,
    limit_price: Price,
    fee_authority: Option<&Pubkey>,
    platform_fee_bps: u16,
) -> Quote {
    let predicted = {
        // Both clocks come from the bank the swap below executes against,
        // so the prediction and the fill are evaluated at the same instant
        // in both expiry domains. A literal here would let the two drift
        // and turn a real divergence into a passing test.
        let now_slot = f.now_slot();
        let now_unix = f.now_unix();
        let data = market_bytes(f);
        let view = MarketView::load(&data).expect("SDK decodes the market account");
        simulate_swap(
            &view,
            side,
            amount_in,
            limit_price,
            now_slot,
            now_unix,
            platform_fee_bps,
        )
    };

    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);
    let quote_before = f.token_balance(&quote_ata);

    let ix = f.swap_ix_with_fee(
        &taker.pubkey(),
        side as u8,
        amount_in,
        limit_price.as_u32(),
        0,
        fee_authority,
        platform_fee_bps,
    );
    f.send_ix(taker, ix).expect("on-chain swap");

    // Buy spends quote for base; Sell spends base for quote.
    let (realized_out, realized_in) = match side {
        SwapSide::Buy => (
            f.token_balance(&base_ata) - base_before,
            quote_before - f.token_balance(&quote_ata),
        ),
        SwapSide::Sell => (
            f.token_balance(&quote_ata) - quote_before,
            base_before - f.token_balance(&base_ata),
        ),
    };

    assert_eq!(
        predicted.out_amount, realized_out,
        "SDK out != on-chain out"
    );
    assert_eq!(predicted.in_amount, realized_in, "SDK in != on-chain in");
    predicted
}

/// The `emit_cpi!` self-CPI framing tag is the 8-byte prefix every event
/// decoder strips before reading a discriminator. The off-chain SDK
/// hand-copies it as a literal (`dropset_sdk::events::EVENT_IX_TAG_LE`) and
/// deliberately doesn't pull in the heavy on-chain `anchor_lang_v2` crate, so
/// nothing there cross-checks the copy against anchor's real constant — and
/// this repo tracks an unreleased anchor-v2 fork. If a fork bump moved the
/// tag, the SDK and the whole indexer would silently decode zero events with
/// no other test failing. This pins the two together at build/test time,
/// mirroring how `dropset_sdk::events` already pins the event discriminators.
#[test]
fn sdk_event_tag_matches_anchor() {
    assert_eq!(
        dropset_sdk::events::EVENT_IX_TAG_LE,
        anchor_lang_v2::event::EVENT_IX_TAG_LE,
        "the SDK's hand-copied EVENT_IX_TAG_LE drifted from anchor's constant"
    );
}

#[test]
fn sdk_layout_decodes_live_market() {
    let f = Fixture::seeded(10_000_000, 10_000_000);
    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");

    // Header mints match what the fixture created.
    assert_eq!(view.header.base_mint, f.base_mint.to_bytes());
    assert_eq!(view.header.quote_mint, f.quote_mint.to_bytes());

    // Vault 0 inventory matches the program's own reader byte-for-byte.
    let onchain = f.vault(0);
    let sdk = &view.sectors()[0];
    assert_eq!(sdk.base_atoms.get(), onchain.base_atoms.get());
    assert_eq!(sdk.quote_atoms.get(), onchain.quote_atoms.get());
    assert_eq!(sdk.total_shares.get(), onchain.total_shares.get());
    // The active DLL walk finds exactly the one seeded vault.
    assert_eq!(view.active_vaults().count(), 1);
}

#[test]
fn sdk_simulate_swap_matches_onchain_buy() {
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    let amount_in: u64 = 1_000_000; // quote atoms (Buy spends quote)
    let taker = f.funded_depositor(0, 2 * amount_in);

    // Buy with no upper price bound, priced at the bank's own clocks
    // (the seeded ladder never expires: both offsets are u32::MAX).
    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, amount_in, Price::INFINITY);
    assert!(q.out_amount > 0, "expected a fill");
    // Consumes ~all the input (a Buy converts quote->base via truncating
    // division, so the last atom may be left unspent).
    assert!(q.in_amount > 0 && q.in_amount <= amount_in);
}

#[test]
fn sdk_simulate_swap_matches_onchain_sell() {
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    let amount_in: u64 = 500_000; // base atoms (Sell spends base)
    let taker = f.funded_depositor(2 * amount_in, 0);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Sell, amount_in, Price::ZERO);
    assert!(q.out_amount > 0, "expected a fill");
}

#[test]
fn sdk_simulate_swap_matches_onchain_with_a_platform_fee() {
    // Close the engine-vs-simulator loop at a *non-zero* fee. Both fees are
    // live here — a taker fee in ppm and a platform fee in bps — so this pins
    // the composition order and the truncation at each step across the two
    // implementations, not just within each.
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 1_000).expect("set taker fee");
    let amount_in: u64 = 1_000_000;
    let taker = f.funded_depositor(0, 2 * amount_in);
    let integrator = f.funded_keypair(common::SIGNER_FUNDING_LAMPORTS);
    let fee_ata = f.platform_fee_ata(&integrator.pubkey(), SwapSide::Buy as u8);

    // `predict_and_execute_with_fee` asserts the SDK's predicted in/out equal
    // the taker's realized deltas — with the fee declared, `out_amount` is net
    // of both fees, so a mismatch in either fee's rounding fails here.
    let q = predict_and_execute_with_fee(
        &mut f,
        &taker,
        SwapSide::Buy,
        amount_in,
        Price::INFINITY,
        Some(&integrator.pubkey()),
        100,
    );
    assert!(q.out_amount > 0, "expected a fill");
    assert!(q.fee_amount > 0, "the taker fee must be live in this case");

    // The simulator's predicted platform fee must equal the atoms the engine
    // actually transferred to the integrator — the half of the composition
    // that the taker's own deltas can't witness.
    assert_eq!(
        q.platform_fee_amount,
        f.token_balance(&fee_ata),
        "SDK platform fee != on-chain platform fee"
    );
}

#[test]
fn sdk_simulate_swap_multi_level_buy() {
    // Two ask levels, 30% of base each, at +0.5% and +2%. A buy big enough
    // to clear the first and bite into the second walks both, so the
    // cross-level price-time priority and per-vault inventory decrement must
    // agree leg-for-leg with the engine.
    let profile = ladder_profile(&[(5_000, 3_000, u32::MAX), (20_000, 3_000, u32::MAX)], &[]);
    let mut f = Fixture::seeded_with(1_000_000, 1_000_000, profile);
    let taker = f.funded_depositor(0, 1_000_000);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 500_000, Price::INFINITY);
    assert!(
        q.legs >= 2,
        "expected a fill across both ask levels, got {}",
        q.legs
    );
}

#[test]
fn sdk_simulate_swap_multi_level_sell() {
    // Symmetric to the multi-level buy: two bid levels, 30% of quote each.
    let profile = ladder_profile(&[], &[(5_000, 3_000, u32::MAX), (20_000, 3_000, u32::MAX)]);
    let mut f = Fixture::seeded_with(1_000_000, 1_000_000, profile);
    let taker = f.funded_depositor(1_000_000, 0);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Sell, 600_000, Price::ZERO);
    assert!(
        q.legs >= 2,
        "expected a fill across both bid levels, got {}",
        q.legs
    );
}

/// Cross-**vault** price-time priority, pinned differentially.
///
/// The DLL walk plus cross-vault sort is the part of the matcher the SDK
/// re-implements rather than shares, and every other case in this file is
/// single-vault — as was every off-chain fixture. `swap.rs` has seven
/// `seeded_two_vaults` cases and not one of them runs the SDK, so the
/// ordering was pinned on-chain and nowhere else.
///
/// Sector 0 is anchored at the *higher* reference, so the better (lower)
/// ask lives in sector 1 and a correct walk crosses sector 1 before sector
/// 0 — the reverse of both DLL order and sector order.
///
/// The take deliberately stops **inside** the second level. That is what
/// makes the ordering observable at all: clearing both levels outright
/// costs the same however they are ordered, so only a fill that runs out
/// of input mid-book prices the two orders differently, and
/// `predict_and_execute`'s in/out equality is then what catches a
/// simulator that walked them the other way.
#[test]
fn sdk_simulate_swap_matches_onchain_across_two_vaults() {
    let high = Price::encode(10_900_000, 0).unwrap().as_u32(); // 1.0900
    let low = Price::encode(10_800_000, 0).unwrap().as_u32(); // 1.0800
    let mut f = Fixture::seeded_two_vaults(high, low);
    // Each vault quotes its full 1_000_000 base at +0.5% of its own
    // reference — ~1.0854 in sector 1, ~1.0954 in sector 0 — so ~2.18M
    // quote would clear both. 1.5M clears the cheaper level and bites into
    // the dearer one.
    let amount_in: u64 = 1_500_000;
    let taker = f.funded_depositor(0, 3_000_000);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, amount_in, Price::INFINITY);
    assert_eq!(q.legs, 2, "expected one ask level from each vault");
    assert_eq!(
        q.in_amount, amount_in,
        "input should be exhausted inside the second vault's level"
    );
}

/// A one-ask-level market whose expiry is finite in exactly one domain and
/// open in the other, quoted after `warp` moves whichever clock the caller
/// means to age out. `predict_and_execute` is the differential: it predicts
/// with the SDK against the pre-swap bytes, runs the real `swap`, and
/// asserts the two agree.
fn expiry_domain_quote(
    expiry_secs: u32,
    expiry_slots: u32,
    warp: impl FnOnce(&mut Fixture),
) -> Quote {
    let profile = dual_expiry_profile(&[(5_000, 10_000, expiry_secs, expiry_slots)], &[]);
    let mut f = Fixture::seeded_with(1_000_000, 1_000_000, profile);
    let taker = f.funded_depositor(0, 1_000_000);
    warp(&mut f);
    predict_and_execute(&mut f, &taker, SwapSide::Buy, 500_000, Price::INFINITY)
}

/// The bound this domain leaves open.
const EXPIRY_OPEN: u32 = u32::MAX;
/// Finite time-in-force per domain, small enough that warping past it keeps
/// the blockhash valid.
const TIF_SLOTS: u32 = 20;
const TIF_SECS: u32 = 60;

/// Dual-domain expiry, pinned **differentially** — the slot half.
///
/// The six expiry vectors are consumed by the native matcher, the WASM
/// binding, the committed binary and the TS wrapper, all four downstream of
/// the same `dropset-interface` matcher, while this file — the only harness
/// that reaches the program — quotes ladders that never expire (see
/// `predict_and_execute`). The program has its own expiry tests and the
/// simulator has its own vectors; what neither side had is a comparison at
/// a *shared* instant, which is the only thing that catches the two
/// drifting apart.
///
/// The dead half on its own would be nearly vacuous — a simulator that
/// always predicted an empty quote would satisfy it, since the engine also
/// fills nothing. The live half against the *same* profile is what makes
/// the pair sharp: both implementations must agree on a real non-zero fill
/// when only the other domain's clock has moved.
#[test]
fn sdk_expiry_slot_domain_matches_onchain() {
    let dead = expiry_domain_quote(EXPIRY_OPEN, TIF_SLOTS, |f| {
        f.warp_slots(TIF_SLOTS as u64 + 10)
    });
    assert_eq!(
        dead.legs, 0,
        "slot bound passed: the book is dead even with the wall bound open"
    );
    assert_eq!(dead.out_amount, 0, "a dead book fills nothing");

    let live = expiry_domain_quote(EXPIRY_OPEN, TIF_SLOTS, |_| {});
    assert!(
        live.out_amount > 0,
        "the same profile must fill while inside its slot bound"
    );
}

/// The wall half of the pair above. This is the halt case the wall datum
/// exists for: slots frozen at a pre-halt value while wall time ran on.
#[test]
fn sdk_expiry_wall_domain_matches_onchain() {
    let dead = expiry_domain_quote(TIF_SECS, EXPIRY_OPEN, |f| f.warp_unix(TIF_SECS as i64 + 10));
    assert_eq!(
        dead.legs, 0,
        "wall bound passed: the book is dead even with the slot bound open"
    );
    assert_eq!(dead.out_amount, 0, "a dead book fills nothing");

    let live = expiry_domain_quote(TIF_SECS, EXPIRY_OPEN, |_| {});
    assert!(
        live.out_amount > 0,
        "the same profile must fill while inside its wall bound"
    );
}

#[test]
fn sdk_simulate_swap_partial_fill_caps_input() {
    // Book far thinner than the taker's input: both the SDK and the engine
    // must cap `in_amount` at the depth actually available.
    let mut f = Fixture::seeded(100_000, 100_000);
    let amount_in: u64 = 50_000_000; // dwarfs the ~100k-base single-level book
    let taker = f.funded_depositor(0, amount_in);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, amount_in, Price::INFINITY);
    assert!(q.out_amount > 0, "expected a fill");
    assert!(
        q.in_amount < amount_in,
        "input should be capped at book depth"
    );
}

#[test]
fn sdk_simulate_swap_limit_price_stops_fill() {
    // Asks at +0.5% (~1.0904) and +5% (~1.1393); a 1.10 limit clears the
    // first level and crosses the second, so exactly one leg fills.
    let profile = ladder_profile(&[(5_000, 3_000, u32::MAX), (50_000, 3_000, u32::MAX)], &[]);
    let mut f = Fixture::seeded_with(1_000_000, 1_000_000, profile);
    let taker = f.funded_depositor(0, 1_000_000);
    let limit = Price::encode(11_000_000, 0).unwrap(); // 1.10

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 1_000_000, limit);
    assert_eq!(
        q.legs, 1,
        "limit should stop the fill after the first level"
    );
    assert!(
        q.in_amount < 1_000_000,
        "second level crossed, input not exhausted"
    );
}

#[test]
fn sdk_simulate_swap_skips_oversize_ask_side_not_the_whole_take() {
    // A flush profile with `size_bps > BPS` can't be written through
    // `set_liquidity_profile` (it bounds the per-side Σ to BPS), but a
    // corrupt account could hold one. The matcher no longer aborts the whole
    // `swap` on it — it throws out just the offending side (zeroing its
    // `remaining`), exactly like an invalid reference price skips a vault —
    // so one bad vault can't DoS every take. Here the ask side is oversized
    // (BPS = 10_000, so 20_000 bps is 200% of the leg); a Buy consumes asks.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_level_size_bps(0, true, 0, 20_000);

    // The Buy's only depth is the corrupted ask side, so it contributes
    // nothing and — with no other vault — the take is an honest no-fill,
    // crucially not an abort. The SDK predicts the same empty quote.
    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");
    let q = simulate_swap(
        &view,
        SwapSide::Buy,
        500_000,
        Price::INFINITY,
        SlotTime::new(1),
        WallTime::new(2),
        0,
    );
    assert_eq!(
        q,
        Quote::default(),
        "the oversized ask side contributes no depth"
    );

    let buyer = f.funded_depositor(0, 500_000);
    let buyer_quote_ata = f.quote_ata(&buyer.pubkey());
    let quote_before = f.token_balance(&buyer_quote_ata);
    f.swap(
        &buyer,
        SwapSide::Buy as u8,
        500_000,
        Price::INFINITY.as_u32(),
        0,
    )
    .expect("swap must not abort on an oversized ask side");
    assert_eq!(
        f.token_balance(&buyer_quote_ata),
        quote_before,
        "no fill, but no abort — the taker's input is untouched"
    );

    // The healthy bid side still matches: a Sell fills, and the SDK predicts
    // it exactly — proving the oversized ask side didn't poison the quote.
    let seller = f.funded_depositor(500_000, 0);
    let sell = predict_and_execute(&mut f, &seller, SwapSide::Sell, 500_000, Price::ZERO);
    assert!(sell.out_amount > 0, "the healthy bid side must still fill");
}

#[test]
fn sdk_buy_unaffected_by_oversize_bid_side() {
    // The old engine flushed *both* sides during book construction, so a
    // corrupt bid level aborted even a Buy (which consumes asks). Under the
    // match-time per-side gate a Buy no longer depends on the bid side's sum:
    // the oversized bids are skipped and the healthy asks fill normally, SDK
    // prediction matching the chain leg-for-leg.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_level_size_bps(0, false, 0, 20_000);

    let buyer = f.funded_depositor(0, 500_000);
    let q = predict_and_execute(&mut f, &buyer, SwapSide::Buy, 500_000, Price::INFINITY);
    assert!(
        q.out_amount > 0,
        "a Buy fills the healthy ask side regardless of a bad bid"
    );
}

#[test]
fn sdk_simulate_swap_rejects_cyclic_vault_list() {
    // A `Vault.next` that points back into the active DLL forms a cycle.
    // The list ops keep the active list acyclic, so this is only reachable
    // from corrupt account bytes — here a self-referential `next` on the
    // seeded vault. The engine bounds its walk by `market.len()` steps and
    // hard-rejects the whole `swap` with `CorruptVaultList`; the SDK's
    // bounded `active_vaults` iterator would instead silently truncate and
    // quote the levels it collected before the budget ran out. The
    // simulator must refuse to quote, mirroring the engine's abort.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_vault_next(0, 0); // sector 0 -> sector 0

    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");
    let q = simulate_swap(
        &view,
        SwapSide::Buy,
        500_000,
        Price::INFINITY,
        SlotTime::new(1),
        WallTime::new(2),
        0,
    );
    assert_eq!(
        q,
        Quote::default(),
        "simulator must reject a cyclic vault list, not quote a partial fill"
    );

    let taker = f.funded_depositor(0, 500_000);
    let err = f
        .swap(
            &taker,
            SwapSide::Buy as u8,
            500_000,
            Price::INFINITY.as_u32(),
            0,
        )
        .expect_err("engine must hard-reject a cyclic vault list");
    common::assert_program_error(&err, dropset::DropsetError::CorruptVaultList);
}

#[test]
fn sdk_simulate_swap_rejects_out_of_range_vault_next() {
    // The other corruption class: a `next` that dangles past the slab tail
    // rather than cycling. Pre-fix, the SDK's `active_vaults` walks the
    // seeded vault, then stops dead when the out-of-range index misses the
    // slab — quoting the one leg it already collected while the engine
    // aborts. On this single-sector slab the dangling step also spends the
    // one-step budget, so both sides actually trip the over-length guard
    // (`steps_remaining > 0` / `steps == 0`) before the separate in-bounds
    // check fires; either way the contract is identical — empty quote vs.
    // `CorruptVaultList`. (`active_dll_is_corrupt`'s in-bounds branch
    // mirrors the engine's own `require!((cur as usize) < len)`, which only
    // a multi-sector list reaches first.)
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_vault_next(0, 9_999); // far past any test slab, and not NULL_SECTOR

    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");
    let q = simulate_swap(
        &view,
        SwapSide::Buy,
        500_000,
        Price::INFINITY,
        SlotTime::new(1),
        WallTime::new(2),
        0,
    );
    assert_eq!(
        q,
        Quote::default(),
        "simulator must reject an out-of-range vault pointer"
    );

    let taker = f.funded_depositor(0, 500_000);
    let err = f
        .swap(
            &taker,
            SwapSide::Buy as u8,
            500_000,
            Price::INFINITY.as_u32(),
            0,
        )
        .expect_err("engine must hard-reject an out-of-range vault pointer");
    common::assert_program_error(&err, dropset::DropsetError::CorruptVaultList);
}

#[test]
fn sdk_simulate_swap_with_taker_fee() {
    // A non-zero taker fee is charged on the output leg. The SDK reads the
    // rate from the market header and must net it out exactly as the engine
    // does — the taker's realized base delta is already net of the fee
    // (which accrues to the market, not to the matched vault).
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    let admin = f.authority.insecure_clone();
    f.set_taker_fee(&admin, 1_000).expect("set taker fee"); // 0.1%
    let taker = f.funded_depositor(0, 2_000_000);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 1_000_000, Price::INFINITY);
    assert!(q.fee_amount > 0, "expected a non-zero taker fee");
    assert!(q.out_amount > 0, "expected a fill");
}
