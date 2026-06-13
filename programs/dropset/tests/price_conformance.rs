//! Program `Price` ⇄ shared conformance vectors.
//!
//! The cross-language codec vectors in `sdk/conformance/price_vectors.json`
//! are generated from `dropset-price-core` and verified by the price-core
//! Rust (`sdk/price-core/tests/conformance.rs`) and the TS client
//! (`sdk/ts/src/conformance.test.ts`). The **program's own** `Price` — the
//! copy in `programs/dropset/src/price.rs` that actually moves funds — is a
//! hand-mirror of that math, so it must be pinned to the same source of
//! truth. This test closes that gap (ENG-476 hole 1): it replays the shared
//! vectors through `dropset::Price`.
//!
//! Scope: the funds-moving paths the program exposes — encoding *validity*
//! (`is_valid`, which gates order-book ordering), the raw-bits round-trip,
//! and the `quote_for_base` / `base_for_quote` ratio math the matcher uses
//! to convert fills between legs. The vectors' `decode.value` (a decoded
//! `f64`) and `encode` (`from_value`, an SDK-only convenience) are not
//! re-checked here: the program's `to_f64` is `#[cfg(test)]`-only and it has
//! no `from_value`, and the decoded value is already exercised end-to-end by
//! the ratio vectors (which decode the significand/exponent internally).

use dropset::Price;
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../sdk/conformance/price_vectors.json"
    );
    let raw = std::fs::read_to_string(path).expect("read price_vectors.json");
    serde_json::from_str(&raw).expect("parse price_vectors.json")
}

fn u32_of(v: &Value, k: &str) -> u32 {
    v[k].as_u64().unwrap() as u32
}
fn u64_of(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap()
}

#[test]
fn decode_vectors() {
    for c in vectors()["decode"].as_array().unwrap() {
        let bits = u32_of(c, "bits");
        let p = Price::from_bits(bits);
        // Raw bits round-trip and the validity classification (which gates
        // order-book ordering) must match the shared vector.
        assert_eq!(p.as_u32(), bits);
        assert_eq!(p.is_valid(), c["valid"].as_bool().unwrap(), "valid({bits})");
    }
}

#[test]
fn quote_for_base_vectors() {
    for c in vectors()["quote_for_base"].as_array().unwrap() {
        let p = Price::from_bits(u32_of(c, "bits"));
        let got = p.quote_for_base(u64_of(c, "base")).min(u64::MAX as u128) as u64;
        assert_eq!(got, u64_of(c, "expected"));
    }
}

#[test]
fn base_for_quote_vectors() {
    for c in vectors()["base_for_quote"].as_array().unwrap() {
        let p = Price::from_bits(u32_of(c, "bits"));
        let got = p.base_for_quote(u64_of(c, "quote")).min(u64::MAX as u128) as u64;
        assert_eq!(got, u64_of(c, "expected"));
    }
}
