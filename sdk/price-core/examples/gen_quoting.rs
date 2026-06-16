//! Generate the cross-language native↔relative **quoting** conformance
//! vectors.
//!
//! `cargo run -p dropset-price-core --example gen_quoting` prints the
//! canonical JSON to stdout (or, with `--write`, writes it back to the
//! checked-in path — see `make conformance-vectors`); it is checked in at
//! `sdk/conformance/quoting_vectors.json` and verified against both
//! quoting forks: the Rust SDK (`sdk/rs/tests/quoting_conformance.rs`,
//! exercising `quoting::NativeBook::to_profile`) and the TS client
//! (`sdk/ts/src/quoting.conformance.test.ts`, exercising
//! `nativeBookToProfileBytes`).
//!
//! The translation a native (absolute-price) book undergoes to become the
//! program's relative `LiquidityProfile` is hand-mirrored in `sdk/rs` and
//! `sdk/ts` with no vector pinning it (ENG-476 hole 2). This generator is
//! the single reference: it encodes the translation **spec** once, using
//! price-core's `Price` for the ratio math, and both forks must reproduce
//! its output.
//!
//! Spec (mirrors `quoting::level_to_relative` in both SDKs):
//! - `ratio_ppm = price.quote_for_base(SCALE) · PPM / reference.quote_for_base(SCALE)`
//! - ask `price_offset = ratio_ppm − PPM`; bid `price_offset = PPM − ratio_ppm`
//! - `size_bps = size · BPS / leg_atoms` (leg = base for asks, quote for bids)
//!
//! All arithmetic is integer and truncating, in the exact operation order
//! the forks use, so the three implementations agree bit-for-bit.

use dropset_price_core::price::Price;
use serde_json::{json, Value};

/// Common scale for decoding a `Price` to an integer value before taking
/// ratios — `value × 10^9`, matching the SDK quoting modules' `SCALE`.
const SCALE: u64 = 1_000_000_000;
/// Parts-per-million denominator for relative price offsets.
const PPM: u128 = 1_000_000;
/// Basis-points denominator for relative level sizes.
const BPS: u128 = 10_000;

/// One native ask/bid level: the absolute price + atom size a leader
/// quotes, sized against `leg_atoms` of the relevant inventory leg.
struct NativeLevel {
    price: Price,
    size: u64,
    expiry_offset: u32,
}

fn nl(significand: u32, exp: i8, size: u64, expiry_offset: u32) -> NativeLevel {
    NativeLevel {
        price: Price::encode(significand, exp).unwrap(),
        size,
        expiry_offset,
    }
}

/// Translate one native level to its expected relative `(price_offset,
/// size_bps)` and pair it with the inputs the forks need to reconstruct it.
fn level_case(lvl: &NativeLevel, reference: Price, leg_atoms: u64, is_ask: bool) -> Value {
    let ref_val = reference.quote_for_base(SCALE);
    let p_val = lvl.price.quote_for_base(SCALE);
    let ratio_ppm = p_val.saturating_mul(PPM) / ref_val;
    let price_offset = if is_ask {
        ratio_ppm - PPM
    } else {
        PPM - ratio_ppm
    };
    let size_bps = lvl.size as u128 * BPS / leg_atoms as u128;
    json!({
        "price_bits": lvl.price.as_u32(),
        "size": lvl.size,
        "expiry_offset": lvl.expiry_offset,
        "price_offset": price_offset as u64,
        "size_bps": size_bps as u64,
    })
}

struct Case {
    reference: Price,
    base_atoms: u64,
    quote_atoms: u64,
    asks: Vec<NativeLevel>,
    bids: Vec<NativeLevel>,
}

fn case_json(c: &Case) -> Value {
    let asks: Vec<Value> = c
        .asks
        .iter()
        .map(|l| level_case(l, c.reference, c.base_atoms, true))
        .collect();
    let bids: Vec<Value> = c
        .bids
        .iter()
        .map(|l| level_case(l, c.reference, c.quote_atoms, false))
        .collect();
    json!({
        "reference_bits": c.reference.as_u32(),
        "base_atoms": c.base_atoms,
        "quote_atoms": c.quote_atoms,
        "asks": asks,
        "bids": bids,
    })
}

fn main() {
    let cases = [
        // Reference 1.0, round offsets and sizes — hand-verifiable.
        // Asks 1.05/1.10 → +50000/+100000 ppm, 2500 bps each of 1_000_000
        // base. Bids 0.95/0.90 → +50000/+100000 ppm, 3000/1000 bps of quote.
        Case {
            reference: Price::encode(10_000_000, 0).unwrap(),
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![
                nl(10_500_000, 0, 250_000, 100),
                nl(11_000_000, 0, 250_000, 100),
            ],
            bids: vec![
                nl(95_000_000, -1, 300_000, 200),
                nl(90_000_000, -1, 100_000, 200),
            ],
        },
        // FX scale: reference EUR/USD 1.0850, asymmetric ladders and
        // inventory. Offsets/sizes computed by the spec above (price-core
        // ratio math) — the forks must reproduce them exactly.
        Case {
            reference: Price::encode(10_850_000, 0).unwrap(),
            base_atoms: 4_000_000,
            quote_atoms: 7_000_000,
            asks: vec![
                nl(10_904_250, 0, 1_000_000, 50), // +5000 ppm
                nl(11_392_500, 0, 800_000, u32::MAX),
            ],
            bids: vec![
                nl(10_795_750, 0, 2_000_000, 50), // -5000 ppm
                nl(10_416_000, 0, 1_500_000, u32::MAX),
            ],
        },
        // Single-level, sub-1.0 reference, level fully consuming its leg
        // (size == leg → 10000 bps, the per-side ceiling).
        Case {
            reference: Price::encode(99_000_000, -1).unwrap(), // 0.99
            base_atoms: 500_000,
            quote_atoms: 500_000,
            asks: vec![nl(10_098_000, 0, 500_000, 10)], // 1.0098 → +20000/... per spec
            bids: vec![nl(97_020_000, -1, 500_000, 10)], // 0.9702
        },
    ];
    let cases: Vec<Value> = cases.iter().map(case_json).collect();
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-price-core --example gen_quoting`. Do not edit by hand. Verified against the Rust SDK quoting fork (sdk/rs/tests/quoting_conformance.rs) and the TS fork (sdk/ts/src/quoting.conformance.test.ts). Each level lists its native inputs (price_bits, size, expiry_offset) and the expected relative outputs (price_offset in ppm, size_bps); all integer math is truncating.",
        "cases": cases,
    });
    emit(&doc, "quoting_vectors.json");
}

/// Print the canonical pretty JSON to stdout, or — with `--write` — write
/// it to the checked-in `sdk/conformance/<file>` so `make
/// conformance-vectors` can regenerate the vectors without a shell
/// redirect. The trailing newline matches `println!` either way, so the
/// CI freshness gate sees identical bytes.
fn emit(doc: &Value, file: &str) {
    let json = serde_json::to_string_pretty(doc).unwrap();
    if std::env::args().any(|a| a == "--write") {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../conformance/");
        std::fs::write(format!("{dir}{file}"), format!("{json}\n")).unwrap();
    } else {
        println!("{json}");
    }
}
