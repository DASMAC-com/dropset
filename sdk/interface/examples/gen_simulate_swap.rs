//! Generate the cross-language `simulate_swap` conformance vectors.
//!
//! `cargo run -p dropset-interface --example gen_simulate_swap` prints the
//! canonical JSON to stdout (or, with `--write`, writes it back to the
//! checked-in path — see `make conformance-vectors`); it lands at
//! `sdk/conformance/simulate_swap_vectors.json` and is verified against the
//! WASM `simulate_swap` binding (`sdk/interface/tests/wasm_conformance.rs`,
//! run under `wasm-pack test --node` in .github/workflows/sdk.yml).
//!
//! Why these vectors exist: `matching::simulate_swap` — the native book
//! simulator — is already pinned to the on-chain engine by
//! `programs/dropset/tests/sdk_conformance.rs` (it replays the real `swap`
//! in litesvm and asserts the SDK's prediction equals the realized fill).
//! What that test cannot reach is the **WASM wrapper** the TS client
//! actually calls: `wasm::simulate_swap` decodes a raw market-account byte
//! slice, dispatches `side: u8` to the matcher, and marshals the `Quote`
//! back across the wasm-bindgen boundary — none of which runs on the host.
//! This generator captures a representative market as the exact bytes that
//! binding consumes, plus the `Quote` each input yields from the native
//! matcher, so the wasm test can prove the binding reproduces it. The chain
//! is: wasm binding == native matcher (the wasm test) ∧ native matcher ==
//! engine (sdk_conformance.rs) ⟹ the binding matches the engine.
//!
//! Fixture fidelity: the market is built straight from the `layout` mirror
//! structs and `bytemuck`-cast to bytes, so it is self-consistent with the
//! decoder by construction. It deliberately does not re-prove that the
//! mirror matches what the program writes on-chain — that axis is owned by
//! `sdk_conformance.rs` against live litesvm bytes. Here the only question
//! is whether the wasm binding agrees with the native matcher over one
//! fixed buffer, so any valid buffer suffices and a synthetic one keeps the
//! generator solana-free and in-crate.

use bytemuck::{bytes_of, cast_slice, Zeroable};
use dropset_interface::layout::{
    MarketHeader, MarketView, Position, ReferencePrice, Vault, ACCOUNT_DISCRIMINATOR_LEN,
    NULL_SECTOR, VAULT_ALIGN,
};
use dropset_interface::matching::{simulate_swap, SwapSide};
use dropset_interface::price::Price;
use serde_json::{json, Value};

/// Taker fee in ppm retained on the output leg (0.1%), so a Buy exercises
/// the `fee_amount` path rather than leaving it at zero.
const TAKER_FEE_PPM: u16 = 1_000;
/// Ample per-vault inventory — large enough that book depth, not vault
/// balance, bounds every fill in the cases below.
const INVENTORY: u64 = 10_000_000;

/// One live `remaining` book level: an absolute price, an atom size, and an
/// expiry slot (`u32::MAX` = never).
fn position(significand: u32, exp: i8, size: u64) -> Position {
    Position {
        price: Price::encode(significand, exp).unwrap().as_u32().into(),
        size: size.into(),
        expires_at: u32::MAX.into(),
    }
}

/// Build the representative market and serialize it to the exact account
/// byte buffer `MarketView::load` (and thus the wasm binding) expects:
/// discriminator, header, `u32` slab length, alignment pad, then the
/// `Vault` sectors. One active vault (sector 0) carries a live EUR/USD book
/// in its `remaining` positions (no flush armed, so the matcher reads them
/// directly): two ask levels and two bid levels, enough for cross-level
/// fills on both sides and a limit that stops mid-book.
fn market_data() -> Vec<u8> {
    let mut header = MarketHeader::zeroed();
    header.head = 0u32.into(); // sector 0 is the (only) active vault
    header.tombstone_head = NULL_SECTOR.into();
    header.free_head = NULL_SECTOR.into();
    header.active_count = 1u32.into();
    header.taker_fee = TAKER_FEE_PPM.into();
    header.base_mint = [2u8; 32];
    header.quote_mint = [3u8; 32];

    let mut v = Vault::zeroed();
    v.next = NULL_SECTOR.into();
    v.prev = NULL_SECTOR.into();
    v.leader = [1u8; 32]; // non-zero leader => not a free sector
    v.reference_price = ReferencePrice {
        stamp: 1u64.into(), // nonce 1, FLUSH_BIT clear => read `remaining`
        price: Price::encode(10_850_000, 0).unwrap().as_u32().into(), // 1.0850
        quote_slot: 0u32.into(),
    };
    v.base_atoms = INVENTORY.into();
    v.quote_atoms = INVENTORY.into();
    // Asks (consumed by a Buy): 1.0904 then 1.1393.
    v.remaining.asks[0] = position(10_904_000, 0, 1_000_000);
    v.remaining.asks[1] = position(11_393_000, 0, 800_000);
    // Bids (consumed by a Sell): 1.0796 then 1.0416.
    v.remaining.bids[0] = position(10_796_000, 0, 2_000_000);
    v.remaining.bids[1] = position(10_416_000, 0, 1_500_000);

    let vaults = [v];
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0u8; ACCOUNT_DISCRIMINATOR_LEN]); // discriminator (load skips it)
    buf.extend_from_slice(bytes_of(&header));
    buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
    // The slab aligns the first sector to the on-chain Vault align — pad to
    // it, matching `MarketView::load`'s `items_start` computation.
    while !buf.len().is_multiple_of(VAULT_ALIGN) {
        buf.push(0);
    }
    buf.extend_from_slice(cast_slice(&vaults));
    buf
}

/// One swap input + the `Quote` the native matcher returns for it.
struct Case {
    name: &'static str,
    side: SwapSide,
    amount_in: u64,
    limit: Price,
    current_slot: u32,
}

fn case_json(view: &MarketView<'_>, c: &Case) -> Value {
    let q = simulate_swap(view, c.side, c.amount_in, c.limit, c.current_slot);
    json!({
        "name": c.name,
        "side": c.side as u8,
        "amount_in": c.amount_in,
        "limit_price_bits": c.limit.as_u32(),
        "current_slot": c.current_slot,
        "expected": {
            "in_amount": q.in_amount,
            "out_amount": q.out_amount,
            "fee_amount": q.fee_amount,
            "legs": q.legs,
        },
    })
}

fn main() {
    let data = market_data();
    let view = MarketView::load(&data).expect("fixture market decodes");

    let cases = [
        // Buy that clears both ask levels — cross-level price-time priority,
        // capped at book depth, and a non-zero taker fee on the output leg.
        Case {
            name: "buy_multi_level",
            side: SwapSide::Buy,
            amount_in: 3_000_000, // quote atoms; dwarfs the ~2.0M-quote ask depth
            limit: Price::INFINITY,
            current_slot: 1,
        },
        // Buy with a 1.10 limit: ask[0] (1.0904) fills, ask[1] (1.1393)
        // crosses — exactly one leg.
        Case {
            name: "buy_limit_stops",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::encode(11_000_000, 0).unwrap(), // 1.10
            current_slot: 1,
        },
        // Small buy fully absorbed by ask[0] — single leg, input not capped.
        Case {
            name: "buy_single_level",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            current_slot: 1,
        },
        // Sell that clears both bid levels — the symmetric cross-level path.
        Case {
            name: "sell_multi_level",
            side: SwapSide::Sell,
            amount_in: 5_000_000, // base atoms; dwarfs the bid depth
            limit: Price::ZERO,
            current_slot: 1,
        },
    ];
    let cases: Vec<Value> = cases.iter().map(|c| case_json(&view, c)).collect();
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-interface --example gen_simulate_swap`. Do not edit by hand. `market_data` is a representative market account's raw bytes (incl. the 8-byte discriminator); each case lists a swap input (side 0=buy/1=sell, amount_in, limit_price_bits, current_slot) and the Quote the native matcher returns. Verified against the WASM binding in sdk/interface/tests/wasm_conformance.rs (wasm::simulate_swap == native matcher); the native matcher is pinned to the on-chain engine by programs/dropset/tests/sdk_conformance.rs.",
        "market_data": data,
        "cases": cases,
    });
    emit(&doc, "simulate_swap_vectors.json");
}

/// Print the canonical pretty JSON to stdout, or — with `--write` — write
/// it to the checked-in `sdk/conformance/<file>` so `make
/// conformance-vectors` can regenerate the vectors without a shell
/// redirect. The trailing newline matches `println!` either way, so the CI
/// freshness gate sees identical bytes.
fn emit(doc: &Value, file: &str) {
    let json = serde_json::to_string_pretty(doc).unwrap();
    if std::env::args().any(|a| a == "--write") {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../conformance/");
        std::fs::write(format!("{dir}{file}"), format!("{json}\n")).unwrap();
    } else {
        println!("{json}");
    }
}
