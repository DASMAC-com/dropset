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
//! `matching.rs` flagged shared matching vectors as an open follow-up
//! (ENG-476 hole 3): the original coverage was a single seeded ladder with
//! two single-leg fills. This file replays the matcher across a scenario
//! set that exercises the paths most likely to drift — cross-level fills,
//! input capped at book depth, a limit price that stops mid-book, and a
//! non-zero taker fee — through both the SDK simulator and a litesvm swap.

mod common;

use anchor_v2_testing::{Keypair, Signer};
use common::fixture::{ladder_profile, Fixture};

use dropset_sdk::layout::MarketView;
use dropset_sdk::matching::{simulate_swap, Quote, SwapSide};
use dropset_sdk::price::Price;

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
/// for a Sell). The seeded ladders never expire, so `current_slot` only has
/// to sit at or past the reference `quote_slot`.
fn predict_and_execute(
    f: &mut Fixture,
    taker: &Keypair,
    side: SwapSide,
    amount_in: u64,
    limit_price: Price,
    current_slot: u32,
) -> Quote {
    let predicted = {
        let data = market_bytes(f);
        let view = MarketView::load(&data).expect("SDK decodes the market account");
        simulate_swap(&view, side, amount_in, limit_price, current_slot)
    };

    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);
    let quote_before = f.token_balance(&quote_ata);

    f.swap(taker, side as u8, amount_in, limit_price.as_u32(), 0)
        .expect("on-chain swap");

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

    // Buy with no upper price bound, current slot 1 (the seeded ladder
    // never expires: expiry_offset = u32::MAX).
    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, amount_in, Price::INFINITY, 1);
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

    let q = predict_and_execute(&mut f, &taker, SwapSide::Sell, amount_in, Price::ZERO, 1);
    assert!(q.out_amount > 0, "expected a fill");
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

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 500_000, Price::INFINITY, 1);
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

    let q = predict_and_execute(&mut f, &taker, SwapSide::Sell, 600_000, Price::ZERO, 1);
    assert!(
        q.legs >= 2,
        "expected a fill across both bid levels, got {}",
        q.legs
    );
}

#[test]
fn sdk_simulate_swap_partial_fill_caps_input() {
    // Book far thinner than the taker's input: both the SDK and the engine
    // must cap `in_amount` at the depth actually available.
    let mut f = Fixture::seeded(100_000, 100_000);
    let amount_in: u64 = 50_000_000; // dwarfs the ~100k-base single-level book
    let taker = f.funded_depositor(0, amount_in);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, amount_in, Price::INFINITY, 1);
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

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 1_000_000, limit, 1);
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
fn sdk_simulate_swap_rejects_oversize_size_bps_consumed_side() {
    // A flush profile with `size_bps > BPS` can't be written through
    // `set_liquidity_profile` (it bounds the per-side Σ to BPS), but a
    // corrupt account could hold one. The engine materializes the book up
    // front and `flush_level_size` hard-rejects the whole `swap`; the SDK
    // simulator must refuse to quote rather than predict a fill the engine
    // will abort. Here the corruption is on the consumed side (a Buy
    // consumes asks). BPS = 10_000, so 20_000 bps is 200% of the leg.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_level_size_bps(0, true, 0, 20_000);

    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");
    let q = simulate_swap(&view, SwapSide::Buy, 500_000, Price::INFINITY, 1);
    assert_eq!(
        q,
        Quote::default(),
        "simulator must reject a corrupt book, not quote a partial fill"
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
        .expect_err("engine must hard-reject size_bps > BPS");
    common::assert_program_error(&err, dropset::DropsetError::LiquidityProfileSizeOverflow);
}

#[test]
fn sdk_simulate_swap_rejects_oversize_size_bps_other_side() {
    // Same contract, corruption on the side the take does *not* consume: a
    // Buy consumes asks, but here a bid level is out of range. The engine
    // flushes both sides during book construction, so it still aborts — and
    // the simulator must too. This guards against a consumed-side-only
    // check that would quote a fill the engine rejects.
    let mut f = Fixture::seeded(1_000_000, 1_000_000);
    f.poke_level_size_bps(0, false, 0, 20_000);

    let data = market_bytes(&f);
    let view = MarketView::load(&data).expect("SDK decodes the market account");
    let q = simulate_swap(&view, SwapSide::Buy, 500_000, Price::INFINITY, 1);
    assert_eq!(
        q,
        Quote::default(),
        "simulator must reject on a bad bid even for a Buy"
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
        .expect_err("engine aborts on a bad bid even for a Buy");
    common::assert_program_error(&err, dropset::DropsetError::LiquidityProfileSizeOverflow);
}

#[test]
fn sdk_simulate_swap_with_taker_fee() {
    // A non-zero taker fee is retained on the output leg. The SDK reads it
    // from the market header and must net it out exactly as the engine does
    // (the realized base delta is already net of the retained fee).
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    f.poke_taker_fee(1_000); // 0.1%
    let taker = f.funded_depositor(0, 2_000_000);

    let q = predict_and_execute(&mut f, &taker, SwapSide::Buy, 1_000_000, Price::INFINITY, 1);
    assert!(q.fee_amount > 0, "expected a non-zero taker fee");
    assert!(q.out_amount > 0, "expected a fill");
}
