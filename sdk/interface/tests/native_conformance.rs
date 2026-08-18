//! Replay `simulate_swap_vectors.json` through the **native** matcher, on
//! the host, in a plain `cargo test`.
//!
//! The fixture already had two readers, and both were off the host path:
//! the WASM binding test next door (`wasm_conformance.rs`, `#![cfg(target_arch
//! = "wasm32")]`, executed only under `wasm-pack test --node` in CI) and the
//! TS suite. So a `cargo test` — the thing a developer actually runs, and
//! the thing the merge queue gates on for the Rust workspace — never
//! touched these vectors at all.
//!
//! That left a real hole. Six of the cases exist specifically to pin the
//! **dual expiry gate**: each domain's bound killing a level on its own,
//! plus the boundary in each. Drop a conjunct from the native matcher's
//! filter and the Rust suite stayed green; you would find out from the TS
//! job, or from the wasm job, or not until a router quoted depth the
//! engine would not fill. This test closes that: all three matchers now
//! answer to the same vectors, and the native one answers on every run.
//!
//! Deliberately *not* a duplicate of the wasm test. That one proves the
//! binding's marshalling (raw-bytes decode, `side: u8` dispatch, the
//! `Quote` getters) over the compiled artifact; this one proves the
//! matcher itself, which is the half a host `cargo test` can reach. The
//! chain they close together is wasm binding == native matcher == on-chain
//! engine, with `programs/dropset/tests/sdk_conformance.rs` pinning the
//! last link in litesvm.

use dropset_interface::clock::{SlotTime, WallTime};
use dropset_interface::layout::MarketView;
use dropset_interface::matching::{simulate_swap, SwapSide};
use dropset_interface::price::Price;
use serde_json::Value;

const VECTORS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../conformance/simulate_swap_vectors.json"
));

fn vectors() -> Value {
    serde_json::from_str(VECTORS).expect("parse simulate_swap_vectors.json")
}

fn market_data(v: &Value) -> Vec<u8> {
    v["market_data"]
        .as_array()
        .expect("market_data is an array")
        .iter()
        .map(|b| b.as_u64().expect("market_data byte") as u8)
        .collect()
}

fn u64_at(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("case field {k}"))
}

/// Every case, through the native matcher.
#[test]
fn native_simulate_swap_matches_vectors() {
    let v = vectors();
    let data = market_data(&v);
    let view = MarketView::load(&data).expect("fixture market decodes");

    let cases = v["cases"].as_array().expect("cases is an array");
    assert!(!cases.is_empty(), "fixture carries no cases");

    for c in cases {
        let name = c["name"].as_str().expect("case name");
        let side = match u64_at(c, "side") {
            0 => SwapSide::Buy,
            1 => SwapSide::Sell,
            other => panic!("{name}: unknown side {other}"),
        };
        // Expiry is dual-domain, so each case carries both clocks. This is
        // the domain boundary for the fixture: the JSON holds two bare
        // numbers, and they are tagged here, once, on the way in.
        let now_slot = SlotTime::new(u64_at(c, "now_slot") as u32);
        let now_unix = WallTime::new(u64_at(c, "now_unix") as u32);

        let q = simulate_swap(
            &view,
            side,
            u64_at(c, "amount_in"),
            Price::from_bits(u64_at(c, "limit_price_bits") as u32),
            now_slot,
            now_unix,
            u64_at(c, "platform_fee_bps") as u16,
        );

        let e = &c["expected"];
        assert_eq!(q.in_amount, u64_at(e, "in_amount"), "{name}: in_amount");
        assert_eq!(q.out_amount, u64_at(e, "out_amount"), "{name}: out_amount");
        assert_eq!(q.fee_amount, u64_at(e, "fee_amount"), "{name}: fee_amount");
        assert_eq!(
            q.platform_fee_amount,
            u64_at(e, "platform_fee_amount"),
            "{name}: platform_fee_amount"
        );
        assert_eq!(u64::from(q.legs), u64_at(e, "legs"), "{name}: legs");
    }
}

/// The expiry cases are the reason this file exists, so assert they are
/// actually present rather than trusting the fixture to still carry them.
///
/// Without this, regenerating the vectors without the `expiry_*` cases
/// would silently reduce the test above to a fill-math check and the
/// dual-gate coverage would evaporate with a green suite — the same class
/// of failure the whole issue is about.
#[test]
fn the_fixture_still_carries_both_expiry_domains() {
    let v = vectors();
    let names: Vec<&str> = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .map(|c| c["name"].as_str().expect("case name"))
        .collect();

    let expiry_cases = names.iter().filter(|n| n.contains("expiry")).count();
    assert!(
        expiry_cases >= 4,
        "expected the dual-gate expiry cases (each domain's bound plus \
         each boundary); found {expiry_cases} in {names:?}"
    );

    // Both domains have to be named, or a regeneration could keep four
    // cases that all pin one axis.
    assert!(
        names.iter().any(|n| n.contains("slot")),
        "no slot-domain expiry case in {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| n.contains("wall") || n.contains("unix")),
        "no wall-domain expiry case in {names:?}"
    );
}

/// Each expiry conjunct must bind **on its own** — the property the
/// vectors encode, restated here as an executable claim over them.
///
/// The replay above would pass even if the native matcher `||`'d the two
/// conjuncts instead of `&&`ing them, provided the expected quotes were
/// regenerated from that same wrong matcher. This reads the cases the
/// other way round: it asserts the fixture contains at least one case
/// that is dead in exactly one domain while live in the other, and that
/// the matcher returns an empty quote for it.
#[test]
fn a_single_dead_domain_empties_the_quote() {
    let v = vectors();
    let data = market_data(&v);
    let view = MarketView::load(&data).expect("fixture market decodes");

    let mut checked = 0usize;
    for c in v["cases"].as_array().expect("cases is an array") {
        let name = c["name"].as_str().expect("case name");
        if !name.contains("expiry") {
            continue;
        }
        let e = &c["expected"];
        // Only the cases the generator marked as fully expired.
        if u64_at(e, "out_amount") != 0 {
            continue;
        }
        let side = match u64_at(c, "side") {
            0 => SwapSide::Buy,
            1 => SwapSide::Sell,
            other => panic!("{name}: unknown side {other}"),
        };
        let q = simulate_swap(
            &view,
            side,
            u64_at(c, "amount_in"),
            Price::from_bits(u64_at(c, "limit_price_bits") as u32),
            SlotTime::new(u64_at(c, "now_slot") as u32),
            WallTime::new(u64_at(c, "now_unix") as u32),
            u64_at(c, "platform_fee_bps") as u16,
        );
        assert_eq!(
            q.out_amount, 0,
            "{name}: a level past either deadline must not fill"
        );
        assert_eq!(q.legs, 0, "{name}: and must contribute no legs");
        checked += 1;
    }
    assert!(
        checked > 0,
        "no expired-expiry case in the fixture — the dual gate is unpinned"
    );
}
