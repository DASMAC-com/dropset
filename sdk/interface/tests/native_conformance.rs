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
use dropset_interface::layout::{MarketView, N_LEVELS};
use dropset_interface::matching::{resting_levels, simulate_swap, SwapSide};
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
    byte_array(&v["market_data"], "market_data")
}

fn byte_array(v: &Value, what: &str) -> Vec<u8> {
    v.as_array()
        .unwrap_or_else(|| panic!("{what} is an array"))
        .iter()
        .map(|b| b.as_u64().unwrap_or_else(|| panic!("{what} byte")) as u8)
        .collect()
}

/// The account bytes a case quotes against. `"primary"` is the top-level
/// `market_data`; every other name is a key in `markets` — the far-out and
/// flush fixtures, which need their own buffers because their books cannot
/// coexist with the primary one (a far-out level behind ample honest depth
/// would be reached by the cases that deliberately dwarf that depth, and a
/// flush-armed vault reads its levels from somewhere else entirely).
fn market_for(v: &Value, name: &str) -> Vec<u8> {
    if name == "primary" {
        return market_data(v);
    }
    let m = &v["markets"][name];
    assert!(!m.is_null(), "case names unknown market `{name}`");
    byte_array(m, name)
}

fn u64_at(v: &Value, k: &str) -> u64 {
    v[k].as_u64().unwrap_or_else(|| panic!("case field {k}"))
}

/// Every case, through the native matcher.
#[test]
fn native_simulate_swap_matches_vectors() {
    let v = vectors();
    let cases = v["cases"].as_array().expect("cases is an array");
    assert!(!cases.is_empty(), "fixture carries no cases");

    for c in cases {
        let name = c["name"].as_str().expect("case name");
        // Each case names its own market, so a far-out or flush case is
        // quoted against the buffer it was generated from rather than
        // against the primary book.
        let data = market_for(&v, c["market"].as_str().expect("case market"));
        let view = MarketView::load(&data).expect("fixture market decodes");
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

    // Each domain must be pinned on its own. Note this cannot be an
    // `any(contains("slot"))` / `any(contains("wall"))` pair: the cross
    // cases name *both* domains (`expiry_slot_dead_wall_live`), so a
    // single such case would satisfy both predicates and six copies of it
    // would pass the count above too. Group by which domain the case is
    // *about* — its prefix — so each axis has to be present on its own.
    let slot_axis = expiry
        .iter()
        .filter(|n| n.starts_with("expiry_slot"))
        .count();
    let wall_axis = expiry
        .iter()
        .filter(|n| n.starts_with("expiry_wall"))
        .count();
    assert_eq!(
        (slot_axis, wall_axis),
        (3, 3),
        "expected three slot-led and three wall-led expiry cases (each \
         domain: the cross case, plus its boundary live and dead); found \
         {expiry:?}"
    );
}

/// The flush market must keep carrying its own two expiry cases, for the
/// same reason the primary market's six are pinned above.
///
/// The flush path computes each deadline from the profile's offsets plus
/// the reference datum, where `remaining` carries absolute deadlines, so it
/// is separate expiry arithmetic reached by separate cases. Losing them
/// would leave the flush materialization pinned only at a live clock, with
/// a green suite — the failure mode this whole file guards against.
#[test]
fn the_fixture_still_carries_the_flush_expiry_cases() {
    let v = vectors();
    let flush: Vec<(&str, u64)> = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .filter(|c| c["market"].as_str() == Some("flush"))
        .map(|c| {
            (
                c["name"].as_str().expect("case name"),
                u64_at(&c["expected"], "out_amount"),
            )
        })
        .filter(|(n, _)| n.starts_with("flush_expiry_"))
        .collect();

    assert_eq!(
        flush.len(),
        2,
        "expected each domain to kill the flush-materialized book on its \
         own; found {flush:?}"
    );
    assert!(
        flush.iter().any(|n| n.0.contains("slot_dead"))
            && flush.iter().any(|n| n.0.contains("wall_dead")),
        "expected one slot-led and one wall-led flush expiry case; found \
         {flush:?}"
    );
    // Names alone would pass a regeneration that turned one of these live
    // while keeping its `_dead` name, so assert the outcome too — the same
    // defense the primary six get from the independent-oracle test below.
    for (name, out) in &flush {
        assert_eq!(*out, 0, "{name}: a `_dead` flush case must fill nothing");
    }

    // And pin the *live* pair, or the loss is worse in the other direction:
    // drop those two and the profile-to-level materialization is exercised
    // only at dead clocks, where it correctly produces nothing — so the
    // arithmetic this market exists to cover (price from `reference` plus a
    // ppm offset, size from `size_bps` of inventory) would go untested with
    // a green suite.
    let filling: Vec<(&str, u64)> = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .filter(|c| c["market"].as_str() == Some("flush"))
        .map(|c| {
            (
                c["name"].as_str().expect("case name"),
                u64_at(&c["expected"], "out_amount"),
            )
        })
        .filter(|(n, _)| !n.starts_with("flush_expiry_"))
        .collect();

    assert_eq!(
        filling.len(),
        2,
        "expected one filling flush case per side; found {filling:?}"
    );
    assert!(
        filling.iter().any(|f| f.0.starts_with("flush_buy"))
            && filling.iter().any(|f| f.0.starts_with("flush_sell")),
        "expected a filling flush case on each side; found {filling:?}"
    );
    for (name, out) in &filling {
        assert!(
            *out > 0,
            "{name}: a flush case at a live clock must materialize a fill"
        );
    }
}

/// The equal-price tie-break must be ordered by **nonce**, asserted against
/// an oracle read from the vaults rather than from the emitted book.
///
/// This is the one property in the fixture with no other home. A `Quote`
/// cannot see it — two levels at one price fill to identical totals in
/// either order — so every `cases` replay is blind to it, and the `books`
/// block is compared only by the TS test against the *committed* wasm
/// binary. That leaves a source-level change to the tie-break invisible
/// until someone runs `make wasm`: measured, keying the sort on `sector`
/// ahead of `nonce` left both this crate's suite and the TS suite green.
///
/// So derive the expectation independently: walk the vaults, group each
/// side's live levels by price, and for any price two vaults both quote,
/// the older nonce must come first in the collected book. Sector index is
/// deliberately not consulted — the fixture gives sector 0 the *newer*
/// quote precisely so that nonce order, sector order and DLL walk order
/// disagree.
#[test]
fn equal_price_levels_are_ordered_by_nonce_not_by_sector() {
    let v = vectors();
    let data = market_data(&v);
    let view = MarketView::load(&data).expect("fixture market decodes");
    // The live clock every fill case quotes at, taken from a live case so
    // this cannot drift from the fixture.
    let live = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .find(|c| c["name"].as_str() == Some("buy_multi_level"))
        .expect("the fixture carries buy_multi_level");
    let now_slot = SlotTime::new(u64_at(live, "now_slot") as u32);
    let now_unix = WallTime::new(u64_at(live, "now_unix") as u32);

    let mut checked = 0usize;
    for (side, is_buy) in [(SwapSide::Buy, true), (SwapSide::Sell, false)] {
        // The oracle: (price bits, nonce, raw size) for every live level,
        // read straight off the vaults.
        let mut quoted: Vec<(u32, u64, u64)> = Vec::new();
        for (_sector, vault) in view.active_vaults() {
            let nonce = vault.reference_price.nonce();
            for i in 0..N_LEVELS {
                let lvl = if is_buy {
                    vault.remaining.asks[i]
                } else {
                    vault.remaining.bids[i]
                };
                if lvl.size.get() == 0 {
                    continue;
                }
                quoted.push((lvl.price.get(), nonce, lvl.size.get()));
            }
        }

        let book = resting_levels(&view, side, now_slot, now_unix);
        for window in book.windows(2) {
            let (a, b) = (&window[0], &window[1]);
            if a.price.as_u32() != b.price.as_u32() {
                continue;
            }
            // Two entries at one price: find each one's nonce by matching
            // the raw size the vault quoted. Asks are reported at their raw
            // size; bids are converted to base, so match on the converted
            // value the same way the collector does.
            let nonce_of = |size: u64| -> u64 {
                quoted
                    .iter()
                    .find(|(p, _, s)| {
                        *p == a.price.as_u32()
                            && if is_buy {
                                *s == size
                            } else {
                                a.price.base_for_quote(*s).min(u64::MAX as u128) as u64 == size
                            }
                    })
                    .map(|(_, n, _)| *n)
                    .unwrap_or_else(|| panic!("no vault quotes size {size} at this price"))
            };
            assert!(
                nonce_of(a.size) < nonce_of(b.size),
                "equal-priced levels are out of nonce order: the entry sized \
                 {} (nonce {}) precedes the one sized {} (nonce {})",
                a.size,
                nonce_of(a.size),
                b.size,
                nonce_of(b.size)
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked, 2,
        "expected one equal-price pair per side to adjudicate; found {checked}"
    );
}

/// The far-out cases must keep *ending the walk* rather than filling.
///
/// Their whole point is that a level the taker cannot afford one output atom
/// at takes nothing, leaving the unspent budget with the taker. The case
/// replay cannot notice if that stops being true — a regeneration moves the
/// expectation along with the outcome — and it cannot notice a mis-route
/// either, since a far-out case emitted against the primary market
/// degenerates into an ordinary fill under the same name. Pin both.
#[test]
fn the_fixture_still_carries_the_far_out_cases() {
    let v = vectors();
    let far: Vec<(&str, u64, u64)> = v["cases"]
        .as_array()
        .expect("cases is an array")
        .iter()
        .filter(|c| c["market"].as_str() == Some("far_out"))
        .map(|c| {
            (
                c["name"].as_str().expect("case name"),
                u64_at(c, "amount_in"),
                u64_at(&c["expected"], "in_amount"),
            )
        })
        .collect();

    assert_eq!(
        far.len(),
        2,
        "expected one far-out case per side; found {far:?}"
    );
    assert!(
        far.iter().any(|f| f.0.starts_with("buy_")) && far.iter().any(|f| f.0.starts_with("sell_")),
        "expected a far-out case on each side; found {far:?}"
    );
    for (name, amount_in, in_amount) in &far {
        assert!(
            *in_amount > 0,
            "{name}: the honest leg must still fill something"
        );
        assert!(
            in_amount * 100 < *amount_in,
            "{name}: consumed {in_amount} of a {amount_in} budget — a \
             far-out level must end the walk, not absorb the remainder"
        );
    }
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

        // Pinning the *outcome* alone would not pin which conjunct
        // produced it: a generator change that made BOTH domains dead in
        // `expiry_slot_dead_wall_live` would keep the outcome correct
        // while the case silently stopped isolating the slot bound. So
        // check the clocks land where the name says, against the very
        // deadlines the matcher will gate on — read through the typed
        // accessors this change introduced.
        let now_slot = SlotTime::new(u64_at(c, "now_slot") as u32);
        let now_unix = WallTime::new(u64_at(c, "now_unix") as u32);
        for (sector, vault) in view.active_vaults() {
            for i in 0..N_LEVELS {
                let lvl = vault.remaining.asks[i];
                if lvl.size.get() == 0 {
                    continue;
                }
                let slot_live = lvl.slot_deadline().is_live_at(now_slot);
                let wall_live = lvl.wall_deadline().is_live_at(now_unix);
                if name.contains("slot_dead") {
                    assert!(
                        !slot_live && wall_live,
                        "{name} (sector {sector} ask {i}): the name says \
                         the slot bound is what kills this level and the \
                         wall bound is still open, but slot_live \
                         ={slot_live} wall_live={wall_live}"
                    );
                } else if name.contains("wall_dead") {
                    assert!(
                        !wall_live && slot_live,
                        "{name} (sector {sector} ask {i}): the name says \
                         the wall bound is what kills this level and the \
                         slot bound is still open, but slot_live \
                         ={slot_live} wall_live={wall_live}"
                    );
                }
            }
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
