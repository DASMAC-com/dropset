//! Engine ⇄ SDK conformance.
//!
//! The off-chain SDK (`dropset-sdk`) hand-mirrors the on-chain account
//! layout and the `swap` matcher. This test proves that mirror is faithful:
//! it stands up a real market in litesvm, decodes the *live* account bytes
//! with the SDK's `MarketView`, predicts a fill with the SDK's
//! `simulate_swap`, then runs the **real** `swap` instruction and asserts
//! the SDK's prediction equals the on-chain realized amounts.
//!
//! If the SDK's `Price` math, flush materialization, or layout ever drift
//! from the program, this fails — the guarantee behind "the SDK does what
//! the engine does".

mod common;

use anchor_v2_testing::Signer;
use common::fixture::Fixture;

use dropset_sdk::layout::MarketView;
use dropset_sdk::matching::{simulate_swap, SwapSide};
use dropset_sdk::price::Price;

/// SDK-decoded snapshot of the live market account.
fn market_bytes(f: &Fixture) -> Vec<u8> {
    f.svm.get_account(&f.market).expect("market account").data
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

    // Predict from the pre-swap snapshot, exactly as a router would.
    let predicted = {
        let data = market_bytes(&f);
        let view = MarketView::load(&data).unwrap();
        // Buy with no upper price bound, current slot 1 (the seeded ladder
        // never expires: expiry_offset = u32::MAX).
        simulate_swap(&view, SwapSide::Buy, amount_in, Price::INFINITY, 1)
    };
    assert!(predicted.out_amount > 0, "expected a fill");
    // Consumes ~all the input (a Buy converts quote->base via truncating
    // division, so the last atom may be left unspent — the on-chain swap
    // does the same, which the equality checks below confirm).
    assert!(predicted.in_amount > 0 && predicted.in_amount <= amount_in);

    // Execute the real swap and measure the taker's realized deltas.
    let taker = f.funded_depositor(0, 2 * amount_in);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);
    let quote_before = f.token_balance(&quote_ata);

    f.swap(
        &taker,
        0, /* Buy */
        amount_in,
        Price::INFINITY.as_u32(),
        0,
    )
    .expect("on-chain swap");

    let realized_out = f.token_balance(&base_ata) - base_before;
    let realized_in = quote_before - f.token_balance(&quote_ata);

    assert_eq!(
        predicted.out_amount, realized_out,
        "SDK out != on-chain out"
    );
    assert_eq!(predicted.in_amount, realized_in, "SDK in != on-chain in");
}

#[test]
fn sdk_simulate_swap_matches_onchain_sell() {
    let mut f = Fixture::seeded(10_000_000, 10_000_000);
    let amount_in: u64 = 500_000; // base atoms (Sell spends base)

    let predicted = {
        let data = market_bytes(&f);
        let view = MarketView::load(&data).unwrap();
        simulate_swap(&view, SwapSide::Sell, amount_in, Price::ZERO, 1)
    };
    assert!(predicted.out_amount > 0, "expected a fill");

    let taker = f.funded_depositor(2 * amount_in, 0);
    let base_ata = f.base_ata(&taker.pubkey());
    let quote_ata = f.quote_ata(&taker.pubkey());
    let base_before = f.token_balance(&base_ata);
    let quote_before = f.token_balance(&quote_ata);

    f.swap(
        &taker,
        1, /* Sell */
        amount_in,
        Price::ZERO.as_u32(),
        0,
    )
    .expect("on-chain swap");

    let realized_out = f.token_balance(&quote_ata) - quote_before;
    let realized_in = base_before - f.token_balance(&base_ata);

    assert_eq!(
        predicted.out_amount, realized_out,
        "SDK out != on-chain out"
    );
    assert_eq!(predicted.in_amount, realized_in, "SDK in != on-chain in");
}
