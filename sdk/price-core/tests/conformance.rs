//! Verify the Rust `Price` math against the shared conformance vectors.
//! The TS client verifies the same file (sdk/ts/src/conformance.test.ts);
//! together they pin both implementations to one source of truth.

use dropset_price_core::price::Price;
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../conformance/price_vectors.json");
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
        let p = Price::from_bits(u32_of(c, "bits"));
        assert_eq!(p.as_u32(), u32_of(c, "bits"));
        assert_eq!(p.is_valid(), c["valid"].as_bool().unwrap());
        match c["value"].as_f64() {
            None => assert!(p.to_f64().is_infinite(), "expected INFINITY sentinel"),
            Some(expected) => {
                let got = p.to_f64();
                let tol = 1e-9 * expected.abs().max(1.0);
                assert!((got - expected).abs() <= tol, "decode {expected} got {got}");
            }
        }
    }
}

#[test]
fn encode_vectors() {
    for c in vectors()["encode"].as_array().unwrap() {
        let value = c["value"].as_f64().unwrap();
        let got = Price::from_value(value).map(|p| p.as_u32());
        let expected = c["bits"].as_u64().map(|b| b as u32);
        assert_eq!(got, expected, "encode {value}");
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
