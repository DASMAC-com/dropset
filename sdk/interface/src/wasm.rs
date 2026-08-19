//! WASM bindings (`wasm` feature) for the TypeScript client — the book
//! simulator half.
//!
//! Exposes [`simulate_swap`] (quote a take) and [`resting_book`] (read the
//! depth the take would fill against) over a market account's raw bytes, so
//! the TS client runs the engine's own slab decode and book reconstruction
//! instead of a hand-mirror. Both entry points share `MarketView::load` and
//! `matching`, which is the point: the byte offsets in `layout.rs` and the
//! materialization rules in `matching.rs` exist in exactly one place, and
//! the TS client cannot drift from them because it does not restate them.
//! The `Price` codec bindings are forwarded from `dropset-math-core` (whose
//! `wasm` feature this crate's `wasm` feature turns on), so a single
//! wasm-pack build over this crate emits both binding sets. Build the JS
//! package with `make wasm` (wasm-pack); see sdk/README.md.
//!
//! `u128` results are saturated to `u64` at the boundary (wasm-bindgen has
//! no `u128`); this is lossless for the FX atom scales the protocol targets.

use wasm_bindgen::prelude::*;

use crate::clock::{SlotTime, WallTime};
use crate::layout::MarketView;
use crate::matching::{
    resting_levels as core_resting_levels, simulate_swap as core_simulate_swap, BookLevel, SwapSide,
};
use crate::price::Price;

/// Result of [`simulate_swap`].
#[wasm_bindgen]
pub struct Quote {
    in_amount: u64,
    out_amount: u64,
    fee_amount: u64,
    platform_fee_amount: u64,
    legs: u32,
}

#[wasm_bindgen]
impl Quote {
    #[wasm_bindgen(getter)]
    pub fn in_amount(&self) -> u64 {
        self.in_amount
    }
    #[wasm_bindgen(getter)]
    pub fn out_amount(&self) -> u64 {
        self.out_amount
    }
    #[wasm_bindgen(getter)]
    pub fn fee_amount(&self) -> u64 {
        self.fee_amount
    }
    #[wasm_bindgen(getter)]
    pub fn platform_fee_amount(&self) -> u64 {
        self.platform_fee_amount
    }
    #[wasm_bindgen(getter)]
    pub fn legs(&self) -> u32 {
        self.legs
    }
}

/// Simulate a take against a market account's raw data (including the
/// 8-byte discriminator). `side`: 0 = buy, 1 = sell. `limit_price_bits`:
/// raw `Price` bits (use the per-side no-bound sentinel to disable).
/// Level expiry is **dual-domain**: `now_slot` is the current slot and
/// `now_unix` the current wall-clock time in unix **seconds**, and a
/// level is shown only while it is inside both of its deadlines. Passing
/// one where the other belongs silently resurrects expired levels (or
/// kills live ones) — this signature is the JS boundary, so the two
/// arrive as bare `u32`s and the domain types are applied on the line
/// below; the TS caller gets the same distinction from the branded types
/// in `sdk/ts/src/clock.ts`. `platform_fee_bps`: the integrator fee
/// the caller will declare on the `swap` instruction — `0` for an
/// unrouted quote. A rate above the market's ceiling yields an all-zero
/// `Quote`, matching the engine's refusal.
#[wasm_bindgen]
pub fn simulate_swap(
    market_data: &[u8],
    side: u8,
    amount_in: u64,
    limit_price_bits: u32,
    now_slot: u32,
    now_unix: u32,
    platform_fee_bps: u16,
) -> Result<Quote, JsError> {
    let view = MarketView::load(market_data)
        .map_err(|e| JsError::new(&alloc_fmt(format_args!("{e:?}"))))?;
    let side = match side {
        0 => SwapSide::Buy,
        1 => SwapSide::Sell,
        _ => return Err(JsError::new("invalid side (expected 0=buy, 1=sell)")),
    };
    let q = core_simulate_swap(
        &view,
        side,
        amount_in,
        Price::from_bits(limit_price_bits),
        SlotTime::new(now_slot),
        WallTime::new(now_unix),
        platform_fee_bps,
    );
    Ok(Quote {
        in_amount: q.in_amount,
        out_amount: q.out_amount,
        fee_amount: q.fee_amount,
        platform_fee_amount: q.platform_fee_amount,
        legs: q.legs,
    })
}

/// Both sides of the reconstructed resting book, as parallel flat arrays.
///
/// Each side is two equal-length arrays rather than an array of level
/// objects: wasm-bindgen maps `Vec<u32>` / `Vec<u64>` onto `Uint32Array` /
/// `BigUint64Array`, so a book crosses the boundary as four typed arrays
/// instead of one JS object per level, each of which the caller would have
/// to `free()`. `prices[i]` and `sizes[i]` describe level `i`.
///
/// `sizes` are **base atoms on both sides** — an ask carries base directly,
/// a bid's matchable quote leg is converted to base at the level price (and
/// saturated to `u64`) by [`resting_levels`](crate::matching::resting_levels)
/// — so the two sides are directly comparable.
#[wasm_bindgen]
pub struct RestingBook {
    bid_prices: Vec<u32>,
    bid_sizes: Vec<u64>,
    ask_prices: Vec<u32>,
    ask_sizes: Vec<u64>,
}

#[wasm_bindgen]
impl RestingBook {
    /// Bid prices as raw `Price` bits, best (highest) first.
    #[wasm_bindgen(getter)]
    pub fn bid_prices(&self) -> Vec<u32> {
        self.bid_prices.clone()
    }
    /// Bid depth in base atoms, aligned with [`Self::bid_prices`].
    #[wasm_bindgen(getter)]
    pub fn bid_sizes(&self) -> Vec<u64> {
        self.bid_sizes.clone()
    }
    /// Ask prices as raw `Price` bits, best (lowest) first.
    #[wasm_bindgen(getter)]
    pub fn ask_prices(&self) -> Vec<u32> {
        self.ask_prices.clone()
    }
    /// Ask depth in base atoms, aligned with [`Self::ask_prices`].
    #[wasm_bindgen(getter)]
    pub fn ask_sizes(&self) -> Vec<u64> {
        self.ask_sizes.clone()
    }
}

/// Split a side's levels into the parallel `(prices, sizes)` arrays the
/// boundary carries.
fn split_side(levels: Vec<BookLevel>) -> (Vec<u32>, Vec<u64>) {
    let mut prices = Vec::with_capacity(levels.len());
    let mut sizes = Vec::with_capacity(levels.len());
    for l in levels {
        prices.push(l.price.as_u32());
        sizes.push(l.size);
    }
    (prices, sizes)
}

/// Reconstruct **both sides** of the resting book from a market account's
/// raw data (including the 8-byte discriminator) — the depth view behind an
/// order-book UI, and the same book [`simulate_swap`] fills against.
///
/// Level expiry is **dual-domain**: `now_slot` is the current slot and
/// `now_unix` the current wall-clock time in unix **seconds**, and a level
/// rests only while it is inside both of its deadlines. Passing one where
/// the other belongs silently resurrects expired levels (or kills live
/// ones) — this signature is the JS boundary, so the two arrive as bare
/// `u32`s and the domain types are applied inside; the TS caller gets the
/// same distinction from the branded types in `sdk/ts/src/clock.ts`.
///
/// Both sides come from one `MarketView::load`, so a UI polling the book
/// pays a single decode per account fetch. An empty side means either no
/// live levels or a book the engine would reject (a corrupt active list) —
/// a router must not show depth the engine won't fill.
///
/// Note the side mapping: `SwapSide::Buy` *takes from* the asks, so the ask
/// side is collected with `Buy` and the bid side with `Sell`.
#[wasm_bindgen]
pub fn resting_book(
    market_data: &[u8],
    now_slot: u32,
    now_unix: u32,
) -> Result<RestingBook, JsError> {
    let view = MarketView::load(market_data)
        .map_err(|e| JsError::new(&alloc_fmt(format_args!("{e:?}"))))?;
    // The JS boundary hands both clocks over as bare `u32`s, so this is
    // where they enter their domains — once, for both sides.
    let now_slot = SlotTime::new(now_slot);
    let now_unix = WallTime::new(now_unix);
    let (ask_prices, ask_sizes) = split_side(core_resting_levels(
        &view,
        SwapSide::Buy,
        now_slot,
        now_unix,
    ));
    let (bid_prices, bid_sizes) = split_side(core_resting_levels(
        &view,
        SwapSide::Sell,
        now_slot,
        now_unix,
    ));
    Ok(RestingBook {
        bid_prices,
        bid_sizes,
        ask_prices,
        ask_sizes,
    })
}

fn alloc_fmt(args: core::fmt::Arguments<'_>) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = s.write_fmt(args);
    s
}
