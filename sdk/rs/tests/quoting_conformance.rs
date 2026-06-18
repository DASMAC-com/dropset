//! Verify the Rust quoting fork against the shared quoting vectors.
//!
//! The native-book → relative-profile translation in `quoting.rs` is
//! hand-mirrored in `quoting.ts`; the TS client verifies the same
//! `sdk/conformance/quoting_vectors.json` (`quoting.conformance.test.ts`).
//! Together they pin both forks to the one reference encoded in the
//! generator (`sdk/math-core/examples/gen_quoting.rs`) — ENG-476 hole 2.

use dropset_sdk::price::Price;
use dropset_sdk::quoting::{NativeBook, NativeLevel};
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../conformance/quoting_vectors.json"
    );
    let raw = std::fs::read_to_string(path).expect("read quoting_vectors.json");
    serde_json::from_str(&raw).expect("parse quoting_vectors.json")
}

fn u32_of(v: &Value, k: &str) -> u32 {
    v[k].as_u64().unwrap() as u32
}

fn native_level(v: &Value) -> NativeLevel {
    NativeLevel {
        price: Price::from_bits(u32_of(v, "price_bits")),
        size: v["size"].as_u64().unwrap(),
        expiry_offset: u32_of(v, "expiry_offset"),
    }
}

#[test]
fn quoting_vectors() {
    let doc = vectors();
    for case in doc["cases"].as_array().unwrap() {
        let reference = Price::from_bits(u32_of(case, "reference_bits"));
        let base_atoms = case["base_atoms"].as_u64().unwrap();
        let quote_atoms = case["quote_atoms"].as_u64().unwrap();

        let asks: Vec<&Value> = case["asks"].as_array().unwrap().iter().collect();
        let bids: Vec<&Value> = case["bids"].as_array().unwrap().iter().collect();
        let book = NativeBook {
            asks: asks.iter().map(|v| native_level(v)).collect(),
            bids: bids.iter().map(|v| native_level(v)).collect(),
        };

        let profile = book
            .to_profile(reference, base_atoms, quote_atoms)
            .expect("native book translates to a relative profile");

        for (i, v) in asks.iter().enumerate() {
            let lvl = &profile.asks[i];
            assert_eq!(
                lvl.price_offset.get(),
                u32_of(v, "price_offset"),
                "ask[{i}] offset"
            );
            assert_eq!(
                lvl.size_bps.get() as u32,
                u32_of(v, "size_bps"),
                "ask[{i}] size_bps"
            );
            assert_eq!(
                lvl.expiry_offset.get(),
                u32_of(v, "expiry_offset"),
                "ask[{i}] expiry"
            );
        }
        for (i, v) in bids.iter().enumerate() {
            let lvl = &profile.bids[i];
            assert_eq!(
                lvl.price_offset.get(),
                u32_of(v, "price_offset"),
                "bid[{i}] offset"
            );
            assert_eq!(
                lvl.size_bps.get() as u32,
                u32_of(v, "size_bps"),
                "bid[{i}] size_bps"
            );
            assert_eq!(
                lvl.expiry_offset.get(),
                u32_of(v, "expiry_offset"),
                "bid[{i}] expiry"
            );
        }
    }
}
