//! WASM bindings (`wasm` feature) for the TypeScript client — the book
//! simulator half.
//!
//! Exposes [`simulate_swap`] over a market account's raw bytes so the TS
//! client runs the engine's book reconstruction instead of a hand-mirror.
//! The `Price` codec bindings are forwarded from `dropset-math-core` (whose
//! `wasm` feature this crate's `wasm` feature turns on), so a single
//! wasm-pack build over this crate emits both binding sets. Build the JS
//! package with `make wasm` (wasm-pack); see sdk/README.md.
//!
//! `u128` results are saturated to `u64` at the boundary (wasm-bindgen has
//! no `u128`); this is lossless for the FX atom scales the protocol targets.

use wasm_bindgen::prelude::*;

use crate::layout::MarketView;
use crate::matching::{simulate_swap as core_simulate_swap, SwapSide};
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
/// kills live ones). `platform_fee_bps`: the integrator fee
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
        now_slot,
        now_unix,
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

fn alloc_fmt(args: core::fmt::Arguments<'_>) -> String {
    use core::fmt::Write;
    let mut s = String::new();
    let _ = s.write_fmt(args);
    s
}
