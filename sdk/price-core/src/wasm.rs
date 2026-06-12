//! WASM bindings (`wasm` feature) for the TypeScript client.
//!
//! Exposes the exact `Price` codec and book-reconstruction math so those
//! clients run the engine's arithmetic instead of a hand-mirror. Build the
//! JS package with `make wasm` (wasm-pack); see sdk/README.md.
//!
//! `u128` results from the ratio math are saturated to `u64` at the
//! boundary (wasm-bindgen has no `u128`); this is lossless for the FX atom
//! scales the protocol targets.

use wasm_bindgen::prelude::*;

use crate::layout::MarketView;
use crate::matching::{simulate_swap as core_simulate_swap, SwapSide};
use crate::price::Price;

/// Encode a decimal price (e.g. `1.085`) to raw `Price` bits, or `None`
/// (JS `undefined`) if out of range.
#[wasm_bindgen]
pub fn price_encode(value: f64) -> Option<u32> {
    Price::from_value(value).map(|p| p.as_u32())
}

/// Decode raw `Price` bits to a number (`0` / `Infinity` for sentinels).
#[wasm_bindgen]
pub fn price_decode(bits: u32) -> f64 {
    Price::from_bits(bits).to_f64()
}

/// Whether `bits` is a valid `Price` encoding.
#[wasm_bindgen]
pub fn price_is_valid(bits: u32) -> bool {
    Price::from_bits(bits).is_valid()
}

/// `base * price`, rounded toward zero (saturated to u64).
#[wasm_bindgen]
pub fn price_quote_for_base(bits: u32, base: u64) -> u64 {
    Price::from_bits(bits).quote_for_base(base).min(u64::MAX as u128) as u64
}

/// `quote / price`, rounded toward zero (saturated to u64).
#[wasm_bindgen]
pub fn price_base_for_quote(bits: u32, quote: u64) -> u64 {
    Price::from_bits(bits).base_for_quote(quote).min(u64::MAX as u128) as u64
}

/// Result of [`simulate_swap`].
#[wasm_bindgen]
pub struct Quote {
    in_amount: u64,
    out_amount: u64,
    fee_amount: u64,
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
    pub fn legs(&self) -> u32 {
        self.legs
    }
}

/// Simulate a take against a market account's raw data (including the
/// 8-byte discriminator). `side`: 0 = buy, 1 = sell. `limit_price_bits`:
/// raw `Price` bits (use the per-side no-bound sentinel to disable).
#[wasm_bindgen]
pub fn simulate_swap(
    market_data: &[u8],
    side: u8,
    amount_in: u64,
    limit_price_bits: u32,
    current_slot: u32,
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
        current_slot,
    );
    Ok(Quote {
        in_amount: q.in_amount,
        out_amount: q.out_amount,
        fee_amount: q.fee_amount,
        legs: q.legs,
    })
}

fn alloc_fmt(args: core::fmt::Arguments<'_>) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = s.write_fmt(args);
    s
}
