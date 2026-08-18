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
//!
//! **What the replay is and is not.** `gen_simulate_swap` builds the
//! fixture's `expected` block by calling this same native matcher, so
//! [`native_simulate_swap_matches_vectors`] is self-consistent by
//! construction: break the matcher, regenerate, and it goes green again.
//! That is not a flaw — its job is to catch a matcher change made
//! *without* regenerating, which is the realistic accident, and the
//! independent oracles for the matcher itself are `sdk_conformance.rs`
//! (against the live engine in litesvm) and the wasm test. But it does
//! mean a guard here must not lean on `expected` if it wants to say
//! anything a regeneration cannot erase — which is why
//! [`each_expiry_conjunct_binds_against_an_independent_oracle`] reads the
//! case *name* instead.

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

/// The six expiry cases are the reason this file exists, so assert they
/// are actually present rather than trusting the fixture to still carry
/// them.
///
/// Without this, regenerating the vectors without the `expiry_*` cases
/// would silently reduce the test above to a fill-math check and the
/// dual-gate coverage would evaporate with a green suite — the same class
/// of failure the whole issue is about.
///
/// The count is pinned **exactly**, and the domain predicates are applied
/// to the expiry cases *only*. Both matter: a `>=` bound tolerates losing
/// cases without anyone noticing the prose in `docs/interface.md` has gone
/// stale, and a domain search over *all* names would be satisfied by an
/// unrelated case that merely happens to contain "slot".
#[test]
fn the_fixture_still_carries_both_expiry_domains() {
    let v = vectors();
    let expiry: Vec<&str> = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .map(|c| c["name"].as_str().expect("case name"))
        .filter(|n| n.starts_with("expiry_"))
        .collect();

    assert_eq!(
        expiry.len(),
        6,
        "expected exactly the six dual-gate expiry cases (each domain's \
         bound killing a level on its own, plus each domain's boundary \
         live and dead); found {expiry:?}"
    );

    // Each domain must be pinned on its own, or a regeneration could keep
    // six cases that all exercise one axis.
    assert!(
        expiry.iter().any(|n| n.contains("slot")),
        "no slot-domain expiry case in {expiry:?}"
    );
    assert!(
        expiry.iter().any(|n| n.contains("wall")),
        "no wall-domain expiry case in {expiry:?}"
    );
}

/// Each expiry conjunct must bind **on its own** — asserted against an
/// oracle *independent of the fixture's own `expected` block*.
///
/// This is the one test here that a regeneration cannot launder. The
/// replay above is self-consistent by construction: `gen_simulate_swap`
/// produces `expected` by calling this very matcher, so breaking the
/// matcher and regenerating leaves it green. A guard that then filtered
/// on `expected.out_amount == 0` would inherit exactly that weakness —
/// the broken matcher's non-zero fill would simply not be selected, and
/// the case would go unchecked.
///
/// So the expectation is derived from the case **name**, which the
/// generator assigns from the scenario it is constructing rather than
/// from the result it observes. The rule is the dual gate itself: a level
/// rests only inside **both** deadlines, so a name mentioning `_dead` in
/// *either* domain must not fill, whatever the matcher currently
/// computes. `expiry_slot_dead_wall_live` is dead in the slot domain and
/// live in the wall domain — and is therefore expected empty, which is
/// precisely the conjunction this test exists to pin.
#[test]
fn each_expiry_conjunct_binds_against_an_independent_oracle() {
    let v = vectors();
    let data = market_data(&v);
    let view = MarketView::load(&data).expect("fixture market decodes");

    let mut dead_checked = 0usize;
    let mut live_checked = 0usize;
    for c in v["cases"].as_array().expect("cases is an array") {
        let name = c["name"].as_str().expect("case name");
        if !name.starts_with("expiry_") {
            continue;
        }
        // The oracle: read the scenario out of the name, not the result
        // out of `expected`. Dead in EITHER domain kills the level, so
        // any `_dead` anywhere in the name means empty.
        let expect_empty = name.contains("_dead");
        assert!(
            expect_empty || name.contains("_live"),
            "{name}: an expiry case must say in its name which domains \
             are dead or live — this oracle reads `_dead` / `_live`"
        );

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

        if expect_empty {
            assert_eq!(
                q.out_amount, 0,
                "{name}: a level past either deadline must not fill"
            );
            assert_eq!(q.legs, 0, "{name}: and must contribute no legs");
            dead_checked += 1;
        } else {
            assert!(
                q.out_amount > 0 && q.legs > 0,
                "{name}: a level inside both deadlines must still fill — \
                 an over-eager gate is as wrong as an absent one"
            );
            live_checked += 1;
        }
    }

    // Both directions must actually have been exercised, or the loop
    // proves nothing: all-dead would pass a matcher that never fills, and
    // all-live one that never expires.
    assert!(
        dead_checked > 0 && live_checked > 0,
        "expiry cases must cover both outcomes; saw {dead_checked} dead \
         and {live_checked} live"
    );
}
