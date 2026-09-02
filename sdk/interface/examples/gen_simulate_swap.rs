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
use dropset_interface::clock::{SlotTime, WallTime};
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
/// Platform-fee ceiling in bps stamped onto the fixture market (1%), so the
/// cases below can declare a real integrator fee and exercise fee
/// composition — and so a case declaring more than this exercises the
/// refuse-to-quote path.
const MAX_PLATFORM_FEE_BPS: u16 = 100;
/// Ample per-vault inventory — large enough that book depth, not vault
/// balance, bounds every fill in the cases below.
const INVENTORY: u64 = 10_000_000;

/// The fixture book's wall deadline, in unix seconds. Finite (rather than
/// `u32::MAX`) so the vectors can pin expiry in *each* domain
/// independently — a level dies when either bound passes, and only finite
/// deadlines on both axes can distinguish "wall expired" from "slot
/// expired".
const WALL_DEADLINE: u32 = 1_700_000_600;
/// The fixture book's slot deadline, likewise finite.
const SLOT_DEADLINE: u32 = 1_000;
/// A clock comfortably inside both deadlines — the "live book" baseline
/// every fill case quotes at.
const LIVE_UNIX: u32 = 1_700_000_000;
const LIVE_SLOT: u32 = 1;

/// One live `remaining` book level: an absolute price, an atom size, and
/// the two absolute deadlines it rests inside.
fn position(significand: u32, exp: i8, size: u64) -> Position {
    Position {
        price: Price::encode(significand, exp).unwrap().as_u32().into(),
        size: size.into(),
        expires_at_unix: WALL_DEADLINE.into(),
        expires_at_slot: SLOT_DEADLINE.into(),
    }
}

/// Scaffold one active vault: a non-zero leader (so it is not a free
/// sector), ample inventory, and a live reference price stamped with
/// `stamp` — `FLUSH_BIT` clear, so the matcher reads `remaining`.
/// `next`/`prev` wire it into the active DLL. The walk follows `next` only,
/// but `prev` is set to match what the program writes.
fn vault(stamp: u64, next: u32, prev: u32, leader: [u8; 32]) -> Vault {
    let mut v = Vault::zeroed();
    v.next = next.into();
    v.prev = prev.into();
    v.leader = leader;
    v.reference_price = ReferencePrice {
        stamp: stamp.into(),
        price: Price::encode(10_850_000, 0).unwrap().as_u32().into(), // 1.0850
        quote_slot: 0u32.into(),
        quote_unix: 0u32.into(),
    };
    v.base_atoms = INVENTORY.into();
    v.quote_atoms = INVENTORY.into();
    v
}

/// Build the representative market and serialize it to the exact account
/// byte buffer `MarketView::load` (and thus the wasm binding) expects:
/// discriminator, header, `u32` slab length, alignment pad, then the
/// `Vault` sectors.
///
/// **Two** active vaults (sectors 0 and 1), each carrying a live EUR/USD
/// book in its `remaining` positions. Their levels **interleave** in price
/// on both sides, so the sorted book alternates vaults and a fill walks
/// 0, 1, 0, 1. That is the point: the DLL walk plus cross-vault sort is the
/// part of the matcher the simulator re-implements rather than shares, and
/// a single-vault fixture cannot tell that sort apart from a per-vault
/// walk — every off-chain fixture used to be single-vault, so the ordering
/// was pinned on-chain and nowhere else.
///
/// The deepest level on each side is an **equal-price pair** across the two
/// vaults, which the nonce tie-break orders (older quote first). That
/// ordering is invisible in a `Quote`'s totals — two levels at one price
/// fill to the same amounts either way — so it is the resting-book vectors
/// that pin it, not the swap cases.
fn market_data() -> Vec<u8> {
    let mut header = MarketHeader::zeroed();
    header.head = 0u32.into(); // sector 0 heads the active DLL
    header.tombstone_head = NULL_SECTOR.into();
    header.free_head = NULL_SECTOR.into();
    header.active_count = 2u32.into();
    header.taker_fee = TAKER_FEE_PPM.into();
    header.max_platform_fee = MAX_PLATFORM_FEE_BPS.into();
    header.base_mint = [2u8; 32];
    header.quote_mint = [3u8; 32];

    // Sector 0 — nonce 1, the older quote, so it wins every equal-price tie.
    let mut v0 = vault(1, 1, NULL_SECTOR, [1u8; 32]);
    // Asks (consumed by a Buy): 1.0904, then the 1.1393 tie-break pair.
    v0.remaining.asks[0] = position(10_904_000, 0, 1_000_000);
    v0.remaining.asks[1] = position(11_393_000, 0, 500_000);
    // Bids (consumed by a Sell): 1.0796, then the 1.0416 tie-break pair.
    v0.remaining.bids[0] = position(10_796_000, 0, 2_000_000);
    v0.remaining.bids[1] = position(10_416_000, 0, 1_500_000);

    // Sector 1 — nonce 2, the newer quote. Its inner level on each side
    // sits *between* sector 0's two, which is what interleaves the book.
    let mut v1 = vault(2, NULL_SECTOR, 0, [4u8; 32]);
    v1.remaining.asks[0] = position(10_950_000, 0, 400_000);
    v1.remaining.asks[1] = position(11_393_000, 0, 300_000);
    v1.remaining.bids[0] = position(10_700_000, 0, 800_000);
    v1.remaining.bids[1] = position(10_416_000, 0, 600_000);

    let vaults = [v0, v1];
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
    now_slot: u32,
    now_unix: u32,
    /// Integrator fee the case declares, in bps. `0` for the unrouted
    /// cases; above `MAX_PLATFORM_FEE_BPS` to pin the refusal path.
    platform_fee_bps: u16,
}

fn case_json(view: &MarketView<'_>, c: &Case) -> Value {
    let q = simulate_swap(
        view,
        c.side,
        c.amount_in,
        c.limit,
        SlotTime::new(c.now_slot),
        WallTime::new(c.now_unix),
        c.platform_fee_bps,
    );
    json!({
        "name": c.name,
        "side": c.side as u8,
        "amount_in": c.amount_in,
        "limit_price_bits": c.limit.as_u32(),
        "now_slot": c.now_slot,
        "now_unix": c.now_unix,
        "platform_fee_bps": c.platform_fee_bps,
        "expected": {
            "in_amount": q.in_amount,
            "out_amount": q.out_amount,
            "fee_amount": q.fee_amount,
            "platform_fee_amount": q.platform_fee_amount,
            "legs": q.legs,
        },
    })
}

fn main() {
    let data = market_data();
    let view = MarketView::load(&data).expect("fixture market decodes");

    let cases = [
        // Buy that clears all four ask levels — cross-*vault* price-time
        // priority (the walk alternates sectors 0, 1, 0, 1), capped at book
        // depth, and a non-zero taker fee on the output leg.
        Case {
            name: "buy_multi_level",
            side: SwapSide::Buy,
            amount_in: 3_000_000, // quote atoms; dwarfs the ~2.44M-quote ask depth
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // Buy with a 1.10 limit: the two inner asks (1.0904 in sector 0 and
        // 1.0950 in sector 1) fill, the 1.1393 tie-break pair crosses — so
        // the limit stops the walk two legs in, having taken one level from
        // each vault. A per-vault walk would stop after sector 0's level.
        Case {
            name: "buy_limit_stops",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::encode(11_000_000, 0).unwrap(), // 1.10
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // Small buy fully absorbed by ask[0] — single leg, input not capped.
        Case {
            name: "buy_single_level",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // Sell that clears all four bid levels — the symmetric cross-vault
        // path, walking sectors 0, 1, 0, 1 down the bid side.
        Case {
            name: "sell_multi_level",
            side: SwapSide::Sell,
            amount_in: 5_000_000, // base atoms; dwarfs the 4.9M-base bid depth
            limit: Price::ZERO,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // The same multi-level Buy at the market's full 100 bps ceiling —
        // pins fee *composition* across the language boundary: the taker fee
        // comes off the gross leg, then the platform fee off what remains,
        // each truncating. Paired with `buy_multi_level` above (identical but
        // for the rate) so a cross-language diff shows exactly which fee
        // moved.
        Case {
            name: "buy_multi_level_platform_fee",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: MAX_PLATFORM_FEE_BPS,
        },
        // Symmetric Sell at the ceiling — the platform fee is charged on the
        // quote leg here, so this catches a leg mix-up the Buy case can't.
        Case {
            name: "sell_multi_level_platform_fee",
            side: SwapSide::Sell,
            amount_in: 5_000_000,
            limit: Price::ZERO,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: MAX_PLATFORM_FEE_BPS,
        },
        // A small Buy at 1 bps whose fee rounds down to zero atoms: the
        // integrator earns nothing and the taker keeps the dust. The engine
        // emits no `PlatformFeeEvent` in this case, so the quote must agree
        // that there is no fee rather than round up to one atom.
        Case {
            name: "buy_platform_fee_rounds_to_zero",
            side: SwapSide::Buy,
            amount_in: 5_000, // ~4.5k base out; 1 bps of that is 0.45 atoms
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 1,
        },
        // One bps over the market's ceiling: the engine hard-rejects
        // (`PlatformFeeTooHigh`), so every field of the expected quote is
        // zero. Pins that both languages *refuse* rather than clamp.
        Case {
            name: "buy_platform_fee_over_ceiling",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: MAX_PLATFORM_FEE_BPS + 1,
        },
        // ── Dual-domain expiry ──────────────────────────────────────────
        // Expiry is the min of two independent bounds, so each domain
        // needs its own kill case *and* its own boundary. A single-domain
        // regression (dropping a conjunct, or reading one clock where the
        // other belongs) leaves the other four green and fails exactly one
        // of these.
        //
        // Slot bound passed, wall bound still ahead: the book is dead.
        Case {
            name: "expiry_slot_dead_wall_live",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: SLOT_DEADLINE,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // Wall bound passed, slot bound still ahead: also dead. This is
        // the halt case — slots frozen at a pre-halt value while wall time
        // ran on.
        Case {
            name: "expiry_wall_dead_slot_live",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: WALL_DEADLINE,
            platform_fee_bps: 0,
        },
        // Boundary, slot domain. The gate is `expires_at_slot <= now`, so the
        // deadline slot itself is dead and the slot before it is live —
        // pinned as a pair so an off-by-one moves exactly one of them.
        Case {
            name: "expiry_slot_boundary_dead",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            now_slot: SLOT_DEADLINE,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        Case {
            name: "expiry_slot_boundary_live",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            now_slot: SLOT_DEADLINE - 1,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // Boundary, wall domain — the same pair on the other axis.
        Case {
            name: "expiry_wall_boundary_dead",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: WALL_DEADLINE,
            platform_fee_bps: 0,
        },
        Case {
            name: "expiry_wall_boundary_live",
            side: SwapSide::Buy,
            amount_in: 500_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: WALL_DEADLINE - 1,
            platform_fee_bps: 0,
        },
    ];
    let cases: Vec<Value> = cases.iter().map(|c| case_json(&view, c)).collect();
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-interface --example gen_simulate_swap`. Do not edit by hand. `market_data` is a representative market account's raw bytes (incl. the 8-byte discriminator); each case lists a swap input (side 0=buy/1=sell, amount_in, limit_price_bits, now_slot, now_unix, platform_fee_bps) and the Quote the native matcher returns. Level expiry is dual-domain: a level rests only while it is inside BOTH its slot deadline and its wall-clock deadline, so the `expiry_*` cases pin each bound independently plus the boundary in each domain. A case whose platform_fee_bps exceeds the market's max_platform_fee expects an all-zero Quote: the engine rejects that swap, so the simulator refuses to quote it rather than clamping the rate. Verified against the WASM binding in sdk/interface/tests/wasm_conformance.rs (wasm::simulate_swap == native matcher); the native matcher is pinned to the on-chain engine by programs/dropset/tests/sdk_conformance.rs.",
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
