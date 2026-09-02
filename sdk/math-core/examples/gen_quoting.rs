//! Generate the cross-language native↔relative **quoting** conformance
//! vectors.
//!
//! `cargo run -p dropset-math-core --example gen_quoting` prints the
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
//! `sdk/ts` with no vector pinning it. This generator is
//! the single reference: it encodes the translation **spec** once, using
//! math-core's `Price` for the ratio math, and both forks must reproduce
//! its output.
//!
//! Spec (mirrors `quoting::level_to_relative` in both SDKs):
//! - `ratio_ppm = price.quote_for_base(SCALE) · PPM / reference.quote_for_base(SCALE)`
//! - ask `price_offset = ratio_ppm − PPM`; bid `price_offset = PPM − ratio_ppm`
//! - `size_bps = size · BPS / leg_atoms` (leg = base for asks, quote for bids)
//! - both expiry offsets are carried through verbatim, one per domain
//!
//! All arithmetic is integer and truncating, in the exact operation order
//! the forks use, so the three implementations agree bit-for-bit. The
//! `cases` deliberately include ratios and sizes that do **not** divide
//! evenly, so a fork that rounded half-up instead of truncating disagrees
//! — a suite of exact divisions would pass either way.
//!
//! Expiry is dual-domain: a level is live only while **both** its slot and
//! wall deadlines hold. The two offsets are independent inputs, so every
//! level below carries a different value in each, and the `UNBOUNDED`
//! sentinel appears in each domain paired with a finite bound in the other.
//! A fork that derived one domain from the other, transposed them, or wrote
//! a zero (which on-chain kills every level) fails these vectors.
//!
//! The happy-path `cases` pin only inputs that translate successfully. The
//! forks are most likely to drift in their **error** handling — the guards
//! that *reject* a level rather than emit one — so a second `rejections`
//! block pins those too: each entry is a native book chosen to
//! trip one guard, tagged with the canonical error both forks must raise.
//! The translation never clamps or saturates; every out-of-range input is
//! rejected, so the vectors assert a rejection (not a clamped output). The
//! tags mirror `quoting::QuotingError`'s variants:
//! - `InvalidReference` — reference is the `ZERO` / `INFINITY` sentinel.
//! - `AskBelowReference` / `BidAboveReference` — `ratio_ppm` lands on the
//!   wrong side of `PPM` (the guards fire before the unsigned subtraction
//!   could underflow).
//! - `OffsetOverflow` — the ppm offset exceeds `u32::MAX`.
//! - `SizeExceedsInventory` — a per-level `size_bps`, the per-side Σ, or a
//!   zero inventory leg breaches the `Σ size_bps ≤ 10000` invariant.

use dropset_math_core::price::Price;
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
///
/// The two expiry offsets are independent domains (see the module docs),
/// so every fixture below gives them **different** values — a fork that
/// wrote one into the other's slot, or zeroed either, disagrees.
struct NativeLevel {
    price: Price,
    size: u64,
    expiry_offset: u32,
    expiry_offset_slots: u32,
}

fn nl(
    significand: u32,
    exp: i8,
    size: u64,
    expiry_offset: u32,
    expiry_offset_slots: u32,
) -> NativeLevel {
    NativeLevel {
        price: Price::encode(significand, exp).unwrap(),
        size,
        expiry_offset,
        expiry_offset_slots,
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
        "expiry_offset_slots": lvl.expiry_offset_slots,
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

/// A native book chosen to trip one translation guard, tagged with the
/// canonical `QuotingError` variant both forks must raise.
/// Unlike [`Case`], the levels carry only native inputs — there is no
/// expected relative output, because the translation rejects.
struct RejectionCase {
    name: &'static str,
    error: &'static str,
    reference: Price,
    base_atoms: u64,
    quote_atoms: u64,
    asks: Vec<NativeLevel>,
    bids: Vec<NativeLevel>,
}

/// Emit one native level as inputs only (no `price_offset` / `size_bps`).
fn native_level_json(lvl: &NativeLevel) -> Value {
    json!({
        "price_bits": lvl.price.as_u32(),
        "size": lvl.size,
        "expiry_offset": lvl.expiry_offset,
        "expiry_offset_slots": lvl.expiry_offset_slots,
    })
}

fn rejection_json(r: &RejectionCase) -> Value {
    let asks: Vec<Value> = r.asks.iter().map(native_level_json).collect();
    let bids: Vec<Value> = r.bids.iter().map(native_level_json).collect();
    json!({
        "name": r.name,
        "error": r.error,
        "reference_bits": r.reference.as_u32(),
        "base_atoms": r.base_atoms,
        "quote_atoms": r.quote_atoms,
        "asks": asks,
        "bids": bids,
    })
}

fn main() {
    let cases = [
        // Reference 1.0, round offsets and sizes — hand-verifiable.
        // Asks 1.05/1.10 → +50000/+100000 ppm, 2500 bps each of 1_000_000
        // base. Bids 0.95/0.90 → +50000/+100000 ppm, 3000/1000 bps of quote.
        // The slot offsets run *opposite* to the wall offsets here (asks
        // 100s/250 slots, bids 200s/125 slots), so a fork that swapped the
        // two domains fails rather than coincidentally agreeing.
        Case {
            reference: Price::encode(10_000_000, 0).unwrap(),
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![
                nl(10_500_000, 0, 250_000, 100, 250),
                nl(11_000_000, 0, 250_000, 100, 275),
            ],
            bids: vec![
                nl(95_000_000, -1, 300_000, 200, 125),
                nl(90_000_000, -1, 100_000, 200, 150),
            ],
        },
        // FX scale: reference EUR/USD 1.0850, asymmetric ladders and
        // inventory. Offsets/sizes computed by the spec above (math-core
        // ratio math) — the forks must reproduce them exactly.
        Case {
            reference: Price::encode(10_850_000, 0).unwrap(),
            base_atoms: 4_000_000,
            quote_atoms: 7_000_000,
            // The second level on each side pairs an *unbounded wall* offset
            // with a finite slot bound, which only holds if the domains are
            // carried independently.
            asks: vec![
                nl(10_904_250, 0, 1_000_000, 50, 400), // +5000 ppm
                nl(11_392_500, 0, 800_000, u32::MAX, 900),
            ],
            bids: vec![
                nl(10_795_750, 0, 2_000_000, 50, 175), // -5000 ppm
                nl(10_416_000, 0, 1_500_000, u32::MAX, 1_200),
            ],
        },
        // Single-level, sub-1.0 reference, level fully consuming its leg
        // (size == leg → 10000 bps, the per-side ceiling).
        Case {
            reference: Price::encode(99_000_000, -1).unwrap(), // 0.99
            base_atoms: 500_000,
            quote_atoms: 500_000,
            // Both sides carry the same wall offset with different slot
            // bounds — and the ask's is the `UNBOUNDED` sentinel while its
            // wall offset is finite, the inverse of case 2's pairing. A fork
            // deriving one domain from the other cannot satisfy both cases.
            asks: vec![nl(10_098_000, 0, 500_000, 10, u32::MAX)], // 1.0098 → +20000/... per spec
            bids: vec![nl(97_020_000, -1, 500_000, 10, 77)],      // 0.9702
        },
        // Inexact ratios: every offset above divides to a whole ppm, so the
        // truncation in `level_to_relative`'s ratio step is unpinned by them
        // — a fork that rounded half-up passes all of them. These do not
        // divide. The ask ratio is 1_000_460.83 ppm, so the offset must floor
        // to 460 (round-half-up gives 461); the bid ratio is 999_262.67 ppm,
        // so its offset must be 738 (round-half-up gives 737). Note the two
        // round in opposite directions, so a fork cannot satisfy both with a
        // single wrong rounding mode. The sizes are inexact too: 700_000 of
        // 3_000_000 base is 2333.33 → 2333 bps, and 900_000 of 7_000_000
        // quote is 1285.71 → 1285 bps.
        Case {
            reference: Price::encode(10_850_000, 0).unwrap(), // 1.085
            base_atoms: 3_000_000,
            quote_atoms: 7_000_000,
            asks: vec![nl(10_855_000, 0, 700_000, 300, 60)],
            bids: vec![nl(10_842_000, 0, 900_000, 300, 90)],
        },
    ];
    // Rejection vectors: each native book trips exactly one translation
    // guard. The forks must reject with the tagged error — the translation
    // never clamps or saturates. See the module docs for the tag set.
    let rejections = [
        // Reference is the ZERO sentinel — no ratio is defined.
        RejectionCase {
            name: "zero reference",
            error: "InvalidReference",
            reference: Price::ZERO,
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![nl(10_500_000, 0, 1_000, 100, 55)],
            bids: vec![],
        },
        // Reference is the INFINITY sentinel — no ratio is defined.
        RejectionCase {
            name: "infinity reference",
            error: "InvalidReference",
            reference: Price::INFINITY,
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![nl(10_500_000, 0, 1_000, 100, 55)],
            bids: vec![],
        },
        // Ask priced below the reference — offsets are unsigned, asks sit
        // above. 0.99 < 1.0.
        RejectionCase {
            name: "ask below reference",
            error: "AskBelowReference",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![nl(99_000_000, -1, 1_000, 100, 55)], // 0.99
            bids: vec![],
        },
        // Bid priced above the reference — the `PPM − ratio_ppm` path the
        // issue flags; the guard fires before the unsigned subtraction can
        // underflow. 1.01 > 1.0.
        RejectionCase {
            name: "bid above reference",
            error: "BidAboveReference",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![],
            bids: vec![nl(10_100_000, 0, 1_000, 100, 55)], // 1.01
        },
        // Ask so far above the reference that the ppm offset overflows u32:
        // 4296× → offset 4_295_000_000 > u32::MAX (4_294_967_295).
        RejectionCase {
            name: "offset overflows u32",
            error: "OffsetOverflow",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![nl(42_960_000, 3, 1_000, 100, 55)], // 4296.0
            bids: vec![],
        },
        // A single level larger than its inventory leg: 1_500_000 of
        // 1_000_000 base → 15000 bps > the 10000 per-side ceiling.
        RejectionCase {
            name: "single level exceeds leg",
            error: "SizeExceedsInventory",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![nl(10_500_000, 0, 1_500_000, 100, 55)],
            bids: vec![],
        },
        // Two individually-valid levels whose Σ exceeds the per-side
        // ceiling: 6000 + 6000 = 12000 bps > 10000.
        RejectionCase {
            name: "per-side sum exceeds ceiling",
            error: "SizeExceedsInventory",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            asks: vec![
                nl(10_500_000, 0, 600_000, 100, 55),
                nl(11_000_000, 0, 600_000, 100, 55),
            ],
            bids: vec![],
        },
        // Zero inventory leg — no size fraction is defined.
        RejectionCase {
            name: "zero inventory leg",
            error: "SizeExceedsInventory",
            reference: Price::encode(10_000_000, 0).unwrap(), // 1.0
            base_atoms: 0,
            quote_atoms: 1_000_000,
            asks: vec![nl(10_500_000, 0, 1_000, 100, 55)],
            bids: vec![],
        },
    ];
    let cases: Vec<Value> = cases.iter().map(case_json).collect();
    let rejections: Vec<Value> = rejections.iter().map(rejection_json).collect();
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-math-core --example gen_quoting`. Do not edit by hand. Verified against the Rust SDK quoting fork (sdk/rs/tests/quoting_conformance.rs) and the TS fork (sdk/ts/src/quoting.conformance.test.ts). `cases` pin successful translations: each level lists its native inputs (price_bits, size, and one expiry_offset per domain — expiry_offset is the wall/seconds bound, expiry_offset_slots the slot bound, always different values so a transposed or zeroed domain fails) and the expected relative outputs (price_offset in ppm, size_bps). `rejections` pin the error paths: each is a native book that trips one guard, tagged with the QuotingError variant both forks must raise (the translation rejects, never clamps). All integer math is truncating.",
        "cases": cases,
        "rejections": rejections,
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
