//! Verify the Rust share / NAV / PnL kernels against the shared share
//! vectors. The TS client verifies the same
//! `sdk/conformance/share_vectors.json` (sdk/ts/src/share.conformance.test.ts);
//! together they pin both forks to the generator
//! (sdk/math-core/examples/gen_share.rs). Integer fields are JSON strings
//! (consensus values exceed JS's 2^53 safe-integer range).

use dropset_math_core::price::Price;
use dropset_math_core::share::{
    compute_pro_rata_slice, crystallize_pnl, isqrt_u128, merge_entry_basis, realize_perf_fee,
    single_leg_basket, BasketError, CrystallizeError,
};
use serde_json::Value;

fn vectors() -> Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../conformance/share_vectors.json"
    );
    let raw = std::fs::read_to_string(path).expect("read share_vectors.json");
    serde_json::from_str(&raw).expect("parse share_vectors.json")
}

fn u128_of(v: &Value, k: &str) -> u128 {
    v[k].as_str().unwrap().parse().unwrap()
}
fn u64_of(v: &Value, k: &str) -> u64 {
    v[k].as_str().unwrap().parse().unwrap()
}
fn i64_of(v: &Value, k: &str) -> i64 {
    v[k].as_str().unwrap().parse().unwrap()
}
fn price_of(v: &Value, k: &str) -> Price {
    Price::from_bits(v[k].as_u64().unwrap() as u32)
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

#[test]
fn isqrt_vectors() {
    for c in vectors()["isqrt"].as_array().unwrap() {
        let n = u128_of(c, "n");
        assert_eq!(isqrt_u128(n), u128_of(c, "expected"), "isqrt({n})");
    }
}

#[test]
fn single_leg_basket_vectors() {
    for c in vectors()["single_leg_basket"].as_array().unwrap() {
        let got = single_leg_basket(
            u64_of(c, "total_shares"),
            u64_of(c, "base_atoms"),
            u64_of(c, "quote_atoms"),
            u64_of(c, "base_in"),
            u64_of(c, "quote_in"),
            u64_of(c, "max_base_in"),
            u64_of(c, "max_quote_in"),
        );
        match got {
            Ok((shares_out, base_in_final, quote_in_final)) => {
                let ok = &c["ok"];
                assert_eq!(shares_out, u64_of(ok, "shares_out"), "shares_out");
                assert_eq!(base_in_final, u64_of(ok, "base_in_final"), "base_in_final");
                assert_eq!(
                    quote_in_final,
                    u64_of(ok, "quote_in_final"),
                    "quote_in_final"
                );
            }
            Err(e) => assert_eq!(basket_err(e), c["err"].as_str().unwrap(), "err"),
        }
    }
}

#[test]
fn pro_rata_slice_vectors() {
    for c in vectors()["pro_rata_slice"].as_array().unwrap() {
        let (slice_base, slice_quote) = compute_pro_rata_slice(
            u64_of(c, "shares_in"),
            u64_of(c, "total_shares"),
            u64_of(c, "base_atoms"),
            u64_of(c, "quote_atoms"),
        );
        assert_eq!(slice_base, u64_of(c, "slice_base"), "slice_base");
        assert_eq!(slice_quote, u64_of(c, "slice_quote"), "slice_quote");
    }
}

#[test]
fn realize_perf_fee_vectors() {
    for c in vectors()["realize_perf_fee"].as_array().unwrap() {
        let r = realize_perf_fee(
            u64_of(c, "base_atoms"),
            u64_of(c, "quote_atoms"),
            u64_of(c, "total_shares"),
            u64_of(c, "leader_shares"),
            u64_of(c, "hwm"),
            c["perf_fee_rate"].as_u64().unwrap() as u32,
        );
        assert_eq!(r.shares_minted, u64_of(c, "shares_minted"), "shares_minted");
        assert_eq!(r.hwm_after, u64_of(c, "hwm_after"), "hwm_after");
        assert_eq!(
            r.total_shares_after,
            u64_of(c, "total_shares_after"),
            "total_shares_after"
        );
        assert_eq!(
            r.leader_shares_after,
            u64_of(c, "leader_shares_after"),
            "leader_shares_after"
        );
    }
}

#[test]
fn crystallize_pnl_vectors() {
    for c in vectors()["crystallize_pnl"].as_array().unwrap() {
        let got = crystallize_pnl(
            u64_of(c, "shares_in"),
            u64_of(c, "shares"),
            u64_of(c, "net_deposits"),
            u64_of(c, "slice_base"),
            u64_of(c, "slice_quote"),
            price_of(c, "entry_ref_bits"),
            price_of(c, "ref_now_bits"),
            i64_of(c, "realized_fx"),
            i64_of(c, "realized_yield"),
            i64_of(c, "realized_pnl"),
        );
        match got {
            Ok(r) => {
                let ok = &c["ok"];
                assert_eq!(r.realized_fx, i64_of(ok, "realized_fx"), "realized_fx");
                assert_eq!(
                    r.realized_yield,
                    i64_of(ok, "realized_yield"),
                    "realized_yield"
                );
                assert_eq!(r.realized_pnl, i64_of(ok, "realized_pnl"), "realized_pnl");
                assert_eq!(r.shares_after, u64_of(ok, "shares_after"), "shares_after");
                assert_eq!(
                    r.net_deposits_after,
                    u64_of(ok, "net_deposits_after"),
                    "net_deposits_after"
                );
                assert_eq!(r.pnl_delta, i64_of(ok, "pnl_delta"), "pnl_delta");
            }
            Err(e) => assert_eq!(crystallize_err(e), c["err"].as_str().unwrap(), "err"),
        }
    }
}

#[test]
fn merge_entry_basis_vectors() {
    for c in vectors()["merge_entry_basis"].as_array().unwrap() {
        let (entry_vps_new, entry_ref_new) = merge_entry_basis(
            u64_of(c, "prior_shares"),
            u64_of(c, "shares_out"),
            u64_of(c, "entry_vps_prev"),
            u64_of(c, "vps_after"),
            price_of(c, "entry_ref_prev_bits"),
            price_of(c, "ref_now_bits"),
        );
        assert_eq!(entry_vps_new, u64_of(c, "entry_vps_new"), "entry_vps_new");
        assert_eq!(
            entry_ref_new.as_u32(),
            c["entry_ref_new_bits"].as_u64().unwrap() as u32,
            "entry_ref_new_bits"
        );
    }
}
