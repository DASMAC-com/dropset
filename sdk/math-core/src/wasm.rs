//! WASM bindings (`wasm` feature) for the TypeScript client — the `Price`
//! codec half.
//!
//! Exposes the exact `Price` codec so those clients run the engine's
//! arithmetic instead of a hand-mirror. The book-simulator binding
//! (`simulate_swap`) lives in `dropset-interface`, which turns this crate's
//! `wasm` feature on so a single wasm-pack build over `dropset-interface`
//! emits both binding sets. Build the JS package with `make wasm`
//! (wasm-pack); see sdk/README.md.
//!
//! `u128` results from the ratio math are saturated to `u64` at the
//! boundary (wasm-bindgen has no `u128`); this is lossless for the FX atom
//! scales the protocol targets.

use wasm_bindgen::prelude::*;

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
    Price::from_bits(bits)
        .quote_for_base(base)
        .min(u64::MAX as u128) as u64
}

/// `quote / price`, rounded toward zero (saturated to u64).
#[wasm_bindgen]
pub fn price_base_for_quote(bits: u32, quote: u64) -> u64 {
    Price::from_bits(bits)
        .base_for_quote(quote)
        .min(u64::MAX as u128) as u64
}
