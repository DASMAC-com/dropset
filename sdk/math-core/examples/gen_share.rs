//! Generate the cross-language share / NAV / PnL conformance vectors.
//!
//! `cargo run -p dropset-math-core --example gen_share` prints the canonical
//! JSON to stdout (or, with `--write`, writes it back to the checked-in path
//! — see `make conformance-vectors`); it is checked in at
//! `sdk/conformance/share_vectors.json` and verified in both Rust
//! (`sdk/math-core/tests/share_conformance.rs`) and TS
//! (`sdk/ts/src/share.conformance.test.ts`).
//!
//! Scope (interface.md §6B): the consensus-critical share kernels — the
//! seeding `isqrt`, single-leg deposit sizing, the pro-rata withdrawal
//! slice, the perf-fee mint, realized-PnL crystallization, and the
//! shares-weighted cost-basis merge. These run on-chain; the off-chain
//! TS SDK fork (`sdk/ts/src/share.ts`) reuses the exact arithmetic, so
//! both forks must reproduce these vectors bit-for-bit.
//!
//! u64 / i64 / u128 fields are serialized as JSON **strings** — consensus
//! values reach `u64::MAX` / `i64::MAX`, beyond JS's 2^53 safe-integer
//! range, so a JSON number would silently lose precision in the TS fork.
//! `Price` fields are raw u32 `bits` (safe as JSON numbers).

use dropset_math_core::price::Price;
use dropset_math_core::share::{
    compute_pro_rata_slice, crystallize_pnl, isqrt_u128, merge_entry_basis, realize_perf_fee,
    single_leg_basket, BasketError, CrystallizeError,
};
use serde_json::{json, Map, Value};

/// Q32.32 representation of `1.0` — the seeded value-per-share / HWM.
const Q32_32_ONE: u64 = 1u64 << 32;

/// Serialize any integer as a JSON string (the precision-safe wire form).
fn s<T: std::fmt::Display>(x: T) -> Value {
    Value::String(x.to_string())
}

fn price(significand: u32, exp: i8) -> Price {
    Price::encode(significand, exp).unwrap()
}

fn basket_err(e: BasketError) -> &'static str {
    match e {
        BasketError::SingleLegRequired => "SingleLegRequired",
        BasketError::MathOverflow => "MathOverflow",
        BasketError::BasketSlippage => "BasketSlippage",
    }
}

fn crystallize_err(e: CrystallizeError) -> &'static str {
    match e {
        CrystallizeError::InsufficientShares => "InsufficientShares",
        CrystallizeError::MathOverflow => "MathOverflow",
    }
}

// ── isqrt ───────────────────────────────────────────────────────────
fn isqrt_cases() -> Vec<Value> {
    let inputs: [u128; 9] = [
        0,
        1,
        2,
        9,
        10_000,
        160_000,
        1_000_000_000_000,
        (u64::MAX as u128) * (u64::MAX as u128),
        (u64::MAX as u128) * (u64::MAX as u128) - 1,
    ];
    inputs
        .iter()
        .map(|&n| json!({ "n": s(n), "expected": s(isqrt_u128(n)) }))
        .collect()
}

// ── single_leg_basket ───────────────────────────────────────────────
#[allow(clippy::too_many_arguments)]
fn slb_case(
    total_shares: u64,
    base_atoms: u64,
    quote_atoms: u64,
    base_in: u64,
    quote_in: u64,
    max_base_in: u64,
    max_quote_in: u64,
) -> Value {
    let mut obj = Map::new();
    obj.insert("total_shares".into(), s(total_shares));
    obj.insert("base_atoms".into(), s(base_atoms));
    obj.insert("quote_atoms".into(), s(quote_atoms));
    obj.insert("base_in".into(), s(base_in));
    obj.insert("quote_in".into(), s(quote_in));
    obj.insert("max_base_in".into(), s(max_base_in));
    obj.insert("max_quote_in".into(), s(max_quote_in));
    match single_leg_basket(
        total_shares,
        base_atoms,
        quote_atoms,
        base_in,
        quote_in,
        max_base_in,
        max_quote_in,
    ) {
        Ok((shares_out, base_in_final, quote_in_final)) => {
            obj.insert(
                "ok".into(),
                json!({
                    "shares_out": s(shares_out),
                    "base_in_final": s(base_in_final),
                    "quote_in_final": s(quote_in_final),
                }),
            );
        }
        Err(e) => {
            obj.insert("err".into(), Value::String(basket_err(e).into()));
        }
    }
    Value::Object(obj)
}

fn single_leg_cases() -> Vec<Value> {
    vec![
        // Neither leg, or both legs supplied — rejected.
        slb_case(100, 1_000, 1_000, 0, 0, u64::MAX, u64::MAX),
        slb_case(100, 1_000, 1_000, 10, 10, u64::MAX, u64::MAX),
        // Floors shares, ceils the basket: 100 base → 10 shares, 100/100.
        slb_case(100, 1_000, 1_000, 100, 0, u64::MAX, u64::MAX),
        // Quote leg: 200 quote into 1000/2000 → 10 shares, basket 100/200.
        slb_case(100, 1_000, 2_000, 0, 200, u64::MAX, u64::MAX),
        // Rounded-up basket exceeds a tight max_base_in.
        slb_case(100, 1_000, 1_000, 100, 0, 50, u64::MAX),
        // A leg too small to buy a whole share floors to zero.
        slb_case(100, 1_000_000, 1_000_000, 1, 0, u64::MAX, u64::MAX),
    ]
}

// ── compute_pro_rata_slice ──────────────────────────────────────────
fn pro_rata_case(shares_in: u64, total_shares: u64, base_atoms: u64, quote_atoms: u64) -> Value {
    let (slice_base, slice_quote) =
        compute_pro_rata_slice(shares_in, total_shares, base_atoms, quote_atoms);
    json!({
        "shares_in": s(shares_in),
        "total_shares": s(total_shares),
        "base_atoms": s(base_atoms),
        "quote_atoms": s(quote_atoms),
        "slice_base": s(slice_base),
        "slice_quote": s(slice_quote),
    })
}

fn pro_rata_cases() -> Vec<Value> {
    vec![
        pro_rata_case(50, 100, 1_000, 2_000),
        pro_rata_case(100, 100, 1_000, 2_000),
        pro_rata_case(1, 3, 10, 10),
        pro_rata_case(25, 100, 0, 4_000),
        // u128 intermediates: full-drain at the atom ceiling round-trips.
        pro_rata_case(u64::MAX, u64::MAX, u64::MAX, u64::MAX),
    ]
}

// ── realize_perf_fee ────────────────────────────────────────────────
fn realize_case(
    base_atoms: u64,
    quote_atoms: u64,
    total_shares: u64,
    leader_shares: u64,
    hwm: u64,
    perf_fee_rate: u32,
) -> Value {
    let r = realize_perf_fee(
        base_atoms,
        quote_atoms,
        total_shares,
        leader_shares,
        hwm,
        perf_fee_rate,
    );
    json!({
        "base_atoms": s(base_atoms),
        "quote_atoms": s(quote_atoms),
        "total_shares": s(total_shares),
        "leader_shares": s(leader_shares),
        "hwm": s(hwm),
        "perf_fee_rate": perf_fee_rate,
        "shares_minted": s(r.shares_minted),
        "hwm_after": s(r.hwm_after),
        "total_shares_after": s(r.total_shares_after),
        "leader_shares_after": s(r.leader_shares_after),
    })
}

fn realize_cases() -> Vec<Value> {
    vec![
        // Unseeded vault — no-op.
        realize_case(0, 0, 0, 0, 0, 100_000),
        // VPS == HWM == 1.0 — no-op.
        realize_case(100, 100, 100, 100, Q32_32_ONE, 100_000),
        // VPS = 4.0 > HWM — 10% perf fee mints shares.
        realize_case(400, 400, 100, 100, Q32_32_ONE, 100_000),
        // Zero fee — HWM advances, no mint.
        realize_case(400, 400, 100, 100, Q32_32_ONE, 0),
        // Drained leg: total_shares > 0 but base·quote == 0 → L == 0 no-op
        // (a distinct branch from the unseeded total_shares == 0 case above).
        realize_case(0, 100, 100, 100, Q32_32_ONE, 100_000),
    ]
}

// ── crystallize_pnl ─────────────────────────────────────────────────
#[allow(clippy::too_many_arguments)]
fn crystallize_case(
    shares_in: u64,
    shares: u64,
    net_deposits: u64,
    slice_base: u64,
    slice_quote: u64,
    entry_ref_price: Price,
    ref_now: Price,
    realized_fx: i64,
    realized_yield: i64,
    realized_pnl: i64,
) -> Value {
    let mut obj = Map::new();
    obj.insert("shares_in".into(), s(shares_in));
    obj.insert("shares".into(), s(shares));
    obj.insert("net_deposits".into(), s(net_deposits));
    obj.insert("slice_base".into(), s(slice_base));
    obj.insert("slice_quote".into(), s(slice_quote));
    obj.insert("entry_ref_bits".into(), json!(entry_ref_price.as_u32()));
    obj.insert("ref_now_bits".into(), json!(ref_now.as_u32()));
    obj.insert("realized_fx".into(), s(realized_fx));
    obj.insert("realized_yield".into(), s(realized_yield));
    obj.insert("realized_pnl".into(), s(realized_pnl));
    match crystallize_pnl(
        shares_in,
        shares,
        net_deposits,
        slice_base,
        slice_quote,
        entry_ref_price,
        ref_now,
        realized_fx,
        realized_yield,
        realized_pnl,
    ) {
        Ok(r) => {
            obj.insert(
                "ok".into(),
                json!({
                    "realized_fx": s(r.realized_fx),
                    "realized_yield": s(r.realized_yield),
                    "realized_pnl": s(r.realized_pnl),
                    "shares_after": s(r.shares_after),
                    "net_deposits_after": s(r.net_deposits_after),
                    "pnl_delta": s(r.pnl_delta),
                }),
            );
        }
        Err(e) => {
            obj.insert("err".into(), Value::String(crystallize_err(e).into()));
        }
    }
    Value::Object(obj)
}

fn crystallize_cases() -> Vec<Value> {
    // The `MathOverflow` err kind is structurally unreachable for valid u64
    // inputs — `net_deposits × shares_in` fits in u128, and
    // `released_basis ≤ net_deposits` whenever `shares_in ≤ shares` — so only
    // the `InsufficientShares` err path gets a vector here.
    let one = price(10_000_000, 0); // 1.0
    let two = price(20_000_000, 0); // 2.0
    vec![
        // Enter at 1.0, withdraw half with ref now 2.0, all-base slice 100:
        // fx +100, yield −400, pnl −300.
        crystallize_case(50, 100, 1_000, 100, 0, one, two, 0, 0, 0),
        // Full drain at a flat 1.0 reference, slice_quote == basis → flat.
        crystallize_case(100, 100, 1_000, 0, 1_000, one, one, 0, 0, 0),
        // A profitable second leg threaded with explicit prior accumulators
        // (75 shares / 750 basis after a prior 25-share quote withdrawal).
        crystallize_case(25, 75, 750, 100, 0, one, two, 0, 0, 0),
        // Overdraw — rejected.
        crystallize_case(20, 10, 100, 0, 0, one, one, 0, 0, 0),
        // slice_quote near u64::MAX → deltas saturate at i64::MAX.
        crystallize_case(1, 2, 0, 0, u64::MAX, one, one, 0, 0, 0),
    ]
}

// ── merge_entry_basis ───────────────────────────────────────────────
fn merge_case(
    prior_shares: u64,
    shares_out: u64,
    entry_vps_prev: u64,
    vps_after: u64,
    entry_ref_prev: Price,
    ref_now: Price,
) -> Value {
    let (entry_vps_new, entry_ref_new) = merge_entry_basis(
        prior_shares,
        shares_out,
        entry_vps_prev,
        vps_after,
        entry_ref_prev,
        ref_now,
    );
    json!({
        "prior_shares": s(prior_shares),
        "shares_out": s(shares_out),
        "entry_vps_prev": s(entry_vps_prev),
        "vps_after": s(vps_after),
        "entry_ref_prev_bits": entry_ref_prev.as_u32(),
        "ref_now_bits": ref_now.as_u32(),
        "entry_vps_new": s(entry_vps_new),
        "entry_ref_new_bits": entry_ref_new.as_u32(),
    })
}

fn merge_cases() -> Vec<Value> {
    let one = price(10_000_000, 0); // 1.0
    let two = price(20_000_000, 0); // 2.0
    vec![
        // Equal-weight top-off → midpoint VPS 1.5, ref blends to ~1.5.
        merge_case(100, 100, Q32_32_ONE, 2 * Q32_32_ONE, one, two),
        // Tiny fresh lot barely moves a large prior position's basis.
        merge_case(999, 1, Q32_32_ONE, 2 * Q32_32_ONE, one, two),
        // Out-of-FX-band fidelity guards for `weighted_average` (the
        // `entry_ref` blend). The VPS arm is exact u128 either way; these
        // pin the Price blend where the TS fork's old `Number(avg)` /
        // plain-bigint paths diverged from Rust.
        //
        // Gap 1 — `Number(avg)` precision. Weights 1_000_000_006 : 1 over
        // refs 6.0000001e7 and 6.0e7 drive `avg` to 60_000_000_999_999_999
        // (`value × 10^9` units, > 2^53), which an f64 round nudges across a
        // 10^9 truncation boundary, flipping the 8th significand digit. The
        // exact bigint path keeps significand 60_000_000.
        merge_case(
            1_000_000_006,
            1,
            Q32_32_ONE,
            2 * Q32_32_ONE,
            price(60_000_001, 7),
            price(60_000_000, 7),
        ),
        // Gap 2 — `u128` saturation. Max-significand prices at the top
        // exponent with `u64::MAX` weights make each `w * v` product exceed
        // `u128::MAX`; Rust's `saturating_mul` clamps it, so the TS fork
        // must too (plain bigint would not).
        merge_case(
            u64::MAX,
            u64::MAX,
            Q32_32_ONE,
            2 * Q32_32_ONE,
            price(99_999_999, 15),
            price(99_999_999, 15),
        ),
    ]
}

fn main() {
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-math-core --example gen_share`. Do not edit by hand. Verified in Rust (sdk/math-core/tests/share_conformance.rs) and TS (sdk/ts/src/share.conformance.test.ts). Integer fields are JSON strings (consensus values exceed JS's 2^53 safe-integer range); `*_bits` are raw u32 Price encodings. `single_leg_basket` / `crystallize_pnl` cases carry either an `ok` object or an `err` kind string.",
        "isqrt": isqrt_cases(),
        "single_leg_basket": single_leg_cases(),
        "pro_rata_slice": pro_rata_cases(),
        "realize_perf_fee": realize_cases(),
        "crystallize_pnl": crystallize_cases(),
        "merge_entry_basis": merge_cases(),
    });
    emit(&doc, "share_vectors.json");
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
