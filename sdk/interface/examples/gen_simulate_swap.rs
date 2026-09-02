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
    Level, MarketHeader, MarketView, Position, ReferencePrice, Vault, ACCOUNT_DISCRIMINATOR_LEN,
    FLUSH_BIT, NULL_SECTOR, VAULT_ALIGN,
};
use dropset_interface::matching::{resting_levels, simulate_swap, SwapSide};
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

/// The common header every fixture market carries: DLL heads set, both fee
/// rates stamped, and `active_count` supplied by the caller.
fn market_header(active_count: u32) -> MarketHeader {
    let mut header = MarketHeader::zeroed();
    header.head = 0u32.into(); // sector 0 heads the active DLL
    header.tombstone_head = NULL_SECTOR.into();
    header.free_head = NULL_SECTOR.into();
    header.active_count = active_count.into();
    header.taker_fee = TAKER_FEE_PPM.into();
    header.max_platform_fee = MAX_PLATFORM_FEE_BPS.into();
    header.base_mint = [2u8; 32];
    header.quote_mint = [3u8; 32];
    header
}

/// Serialize a header + vault slab into the exact account byte buffer
/// `MarketView::load` (and thus the wasm binding) expects: discriminator,
/// header, `u32` slab length, alignment pad, then the `Vault` sectors.
fn serialize_market(header: &MarketHeader, vaults: &[Vault]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&[0u8; ACCOUNT_DISCRIMINATOR_LEN]); // load skips it
    buf.extend_from_slice(bytes_of(header));
    buf.extend_from_slice(&(vaults.len() as u32).to_le_bytes());
    // The slab aligns the first sector to the on-chain Vault align — pad to
    // it, matching `MarketView::load`'s `items_start` computation.
    while !buf.len().is_multiple_of(VAULT_ALIGN) {
        buf.push(0);
    }
    buf.extend_from_slice(cast_slice(vaults));
    buf
}

/// The primary fixture market: **two** active vaults (sectors 0 and 1),
/// each carrying a live EUR/USD book in its `remaining` positions.
///
/// Their levels **interleave** in price on both sides, so the sorted book
/// alternates vaults and a fill walks 0, 1, 0, 1. That is the point: the
/// DLL walk plus cross-vault sort is the part of the matcher the simulator
/// re-implements rather than shares, and a single-vault fixture cannot tell
/// that sort apart from a per-vault walk — every off-chain fixture used to
/// be single-vault, so the ordering was pinned on-chain and nowhere else.
///
/// The deepest level on each side is an **equal-price pair** across the two
/// vaults, which the nonce tie-break orders (older quote first). That
/// ordering is invisible in a `Quote`'s totals — two levels at one price
/// fill to the same amounts either way — so it is the `books` vectors that
/// pin it, not the swap cases.
fn market_data() -> Vec<u8> {
    let header = market_header(2);

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

    serialize_market(&header, &[v0, v1])
}

/// One `remaining` level at an explicit exponent, for prices outside the
/// band [`position`] covers.
fn position_at(significand: u32, exp: i8, size: u64) -> Position {
    Position {
        price: Price::encode(significand, exp).unwrap().as_u32().into(),
        size: size.into(),
        expires_at_unix: WALL_DEADLINE.into(),
        expires_at_slot: SLOT_DEADLINE.into(),
    }
}

/// A market carrying, on **each** side, one honest level with an absurdly
/// far-out level resting behind it.
///
/// There is no program-side bound on a level's price and no oracle coming,
/// so a leader may post any valid price and a far-out level rests
/// harmlessly at the tail of the book. The property that makes exact-in
/// safe is that such a level **ends the walk and takes nothing**: one
/// output atom there costs more than the taker's whole remaining budget, so
/// the budget goes back to the taker rather than being confiscated —
/// `min_out` could not object, because residue never reduces output.
///
/// That property was pinned in-crate for **asks only** (the sole far-out
/// helper builds ask-only markets) and reached no vector at all, so neither
/// the WASM binding, the committed binary, nor the TS wrapper exercised it.
/// The bid side matters more, not less: in ratio terms a bid reaches
/// farther than an ask, since it runs toward zero rather than up.
///
/// Honest depth is deliberately shallow (1_000 atoms) so the taker's budget
/// is not the binding cap — the far-out level, not exhaustion, is what ends
/// the walk.
fn far_out_market_data() -> Vec<u8> {
    let header = market_header(1);
    let mut v = vault(1, NULL_SECTOR, NULL_SECTOR, [1u8; 32]);
    // Honest ask at 1.0904, then the largest representable price behind it.
    v.remaining.asks[0] = position(10_904_000, 0, 1_000);
    v.remaining.asks[1] = position_at(99_999_999, 15, 1_000_000);
    // Honest bid at 1.0796, then a far-out bid behind it — the symmetric
    // case, which had no coverage on either side of the seam. At 1e-9 one
    // quote atom out costs ~1e9 base, so it is unaffordable at any budget
    // these cases use and still ends the walk.
    //
    // Not the *smallest* representable price, deliberately. `resting_levels`
    // reports bid depth base-denominated, so an extreme price saturates
    // that conversion at `u64::MAX` — a value JSON cannot round-trip
    // through a JS number, so the expected book could not be compared in
    // TS at all. 1e-9 keeps the converted depth (1e15 base) inside the
    // exactly-representable range.
    v.remaining.bids[0] = position(10_796_000, 0, 1_000);
    v.remaining.bids[1] = position_at(10_000_000, -9, 1_000_000);
    serialize_market(&header, &[v])
}

/// A market whose vault has the **flush bit armed** and quotes a relative
/// `LiquidityProfile` ladder rather than absolute `remaining` positions.
///
/// Every other fixture here leaves the bit clear, so the matcher reads
/// `remaining` and the profile-to-level materialization — resolving each
/// level's absolute price from `reference ± price_offset` ppm and its size
/// from `size_bps` of the vault's inventory — never reached the WASM
/// binding, the committed binary, or the TS wrapper. That is the state
/// every market is in immediately after a retune, and it was pinned only
/// natively and against the engine.
///
/// The offsets are chosen against the same `quote_unix`/`quote_slot` datum
/// of zero the other fixtures use, so the materialized deadlines land on
/// the same [`WALL_DEADLINE`] / [`SLOT_DEADLINE`] the rest of the vectors
/// quote against.
fn flush_market_data() -> Vec<u8> {
    let header = market_header(1);
    let mut v = vault(1, NULL_SECTOR, NULL_SECTOR, [1u8; 32]);
    v.reference_price.stamp = (1u64 | FLUSH_BIT).into();
    // Asks at +0.5% and +5% of the 1.0850 reference, 25% of base inventory
    // each; bids symmetric on the quote leg. `size_bps` sums to 5000 a
    // side, well inside the per-side ceiling.
    let level = |offset_ppm: u32, size_bps: u16| Level {
        price_offset: offset_ppm.into(),
        size_bps: size_bps.into(),
        expiry_offset_secs: WALL_DEADLINE.into(),
        expiry_offset_slots: SLOT_DEADLINE.into(),
    };
    v.profile.asks[0] = level(5_000, 2_500);
    v.profile.asks[1] = level(50_000, 2_500);
    v.profile.bids[0] = level(5_000, 2_500);
    v.profile.bids[1] = level(50_000, 2_500);
    serialize_market(&header, &[v])
}

/// The market a case quotes against. `PRIMARY` is the two-vault book in
/// `market_data`; the others are the extra buffers in `markets`.
const PRIMARY: &str = "primary";
const FAR_OUT: &str = "far_out";
const FLUSH: &str = "flush";

/// One swap input + the `Quote` the native matcher returns for it. Which
/// market a case quotes against comes from the group it is emitted in, not
/// from the case itself — see `main`.
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

fn case_json(view: &MarketView<'_>, market: &str, c: &Case) -> Value {
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
        "market": market,
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

/// One expected resting book: the four parallel arrays `wasm::resting_book`
/// returns, for one market at one clock.
struct BookCase {
    name: &'static str,
    market: &'static str,
    now_slot: u32,
    now_unix: u32,
}

/// Emit the ordered book both sides of `resting_book` must reproduce.
///
/// This is the only place the cross-vault **order** is asserted directly.
/// A `Quote` cannot see it: two levels at one price fill to the same
/// totals whichever goes first, so the nonce tie-break is invisible in
/// every swap case. Here it is visible, because the sizes ride along in
/// the same order as the prices — the primary book's two equal-priced asks
/// carry different depths, so an inverted tie-break reorders the size
/// array while leaving the price array untouched.
///
/// It is also the only cross-language pin on the `resting_book` binding at
/// all. The committed wasm binary is deliberately excluded from CI's
/// byte-diff (wasm-opt is not byte-reproducible), which makes the behavioral
/// test the sole check on it, and that test imported only `simulate_swap` —
/// leaving `split_side`, the `RestingBook` marshalling, and the
/// Buy-to-asks / Sell-to-bids mapping unverified against the shipped
/// artifact. That is the binding the order-book UI calls, and inverting
/// its side mapping would keep `simulate_swap` green while handing the UI
/// an inverted ladder.
fn book_json(view: &MarketView<'_>, c: &BookCase) -> Value {
    let now_slot = SlotTime::new(c.now_slot);
    let now_unix = WallTime::new(c.now_unix);
    let asks = resting_levels(view, SwapSide::Buy, now_slot, now_unix);
    let bids = resting_levels(view, SwapSide::Sell, now_slot, now_unix);
    let prices = |ls: &[dropset_interface::matching::BookLevel]| -> Vec<u32> {
        ls.iter().map(|l| l.price.as_u32()).collect()
    };
    let sizes = |ls: &[dropset_interface::matching::BookLevel]| -> Vec<u64> {
        ls.iter().map(|l| l.size).collect()
    };
    json!({
        "name": c.name,
        "market": c.market,
        "now_slot": c.now_slot,
        "now_unix": c.now_unix,
        "ask_prices": prices(&asks),
        "ask_sizes": sizes(&asks),
        "bid_prices": prices(&bids),
        "bid_sizes": sizes(&bids),
    })
}

fn main() {
    let data = market_data();
    let view = MarketView::load(&data).expect("fixture market decodes");
    let far_out_data = far_out_market_data();
    let far_out_view = MarketView::load(&far_out_data).expect("far-out market decodes");
    let flush_data = flush_market_data();
    let flush_view = MarketView::load(&flush_data).expect("flush market decodes");

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
    // ── Far-out levels ──────────────────────────────────────────────────
    // A level the taker cannot afford one output atom at ends the walk and
    // takes **nothing**, even though honest depth filled ahead of it. Both
    // sides, because the bid side had no coverage anywhere and reaches
    // farther in ratio terms than the ask side does.
    let far_out_cases = [
        Case {
            name: "buy_far_out_ask_ends_walk",
            side: SwapSide::Buy,
            // Dwarfs the 1_000-atom honest ask, so exhaustion cannot be
            // what stops the walk — the far-out level behind it is.
            amount_in: 5_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        Case {
            name: "sell_far_out_bid_ends_walk",
            side: SwapSide::Sell,
            amount_in: 5_000_000,
            limit: Price::ZERO,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
    ];
    // ── Flush materialization ───────────────────────────────────────────
    // The same two cross-level takes, but against a vault whose flush bit
    // is armed, so each level's price and size are materialized from the
    // relative profile instead of read from `remaining`.
    let flush_cases = [
        Case {
            name: "flush_buy_multi_level",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        Case {
            name: "flush_sell_multi_level",
            side: SwapSide::Sell,
            amount_in: 3_000_000,
            limit: Price::ZERO,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        // The flush path has its own expiry arithmetic — the deadline is
        // materialized from the profile's offsets plus the reference datum,
        // where `remaining` carries absolute deadlines — so each domain
        // needs a kill case here too rather than inheriting the primary
        // market's.
        Case {
            name: "flush_expiry_slot_dead_wall_live",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: SLOT_DEADLINE,
            now_unix: LIVE_UNIX,
            platform_fee_bps: 0,
        },
        Case {
            name: "flush_expiry_wall_dead_slot_live",
            side: SwapSide::Buy,
            amount_in: 3_000_000,
            limit: Price::INFINITY,
            now_slot: LIVE_SLOT,
            now_unix: WALL_DEADLINE,
            platform_fee_bps: 0,
        },
    ];
    let cases: Vec<Value> = cases
        .iter()
        .map(|c| case_json(&view, PRIMARY, c))
        .chain(
            far_out_cases
                .iter()
                .map(|c| case_json(&far_out_view, FAR_OUT, c)),
        )
        .chain(flush_cases.iter().map(|c| case_json(&flush_view, FLUSH, c)))
        .collect();
    // ── Expected resting books ──────────────────────────────────────────
    // One live clock per market, plus each expiry domain against the
    // primary book. The live primary entry is what pins the cross-vault
    // order and the equal-price tie-break.
    let books = [
        BookCase {
            name: "primary_live",
            market: PRIMARY,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
        },
        BookCase {
            name: "primary_slot_dead",
            market: PRIMARY,
            now_slot: SLOT_DEADLINE,
            now_unix: LIVE_UNIX,
        },
        BookCase {
            name: "primary_wall_dead",
            market: PRIMARY,
            now_slot: LIVE_SLOT,
            now_unix: WALL_DEADLINE,
        },
        BookCase {
            name: "far_out_live",
            market: FAR_OUT,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
        },
        BookCase {
            name: "flush_live",
            market: FLUSH,
            now_slot: LIVE_SLOT,
            now_unix: LIVE_UNIX,
        },
    ];
    let books: Vec<Value> = books
        .iter()
        .map(|b| {
            let v = match b.market {
                FAR_OUT => &far_out_view,
                FLUSH => &flush_view,
                _ => &view,
            };
            book_json(v, b)
        })
        .collect();
    let doc = json!({
        "_comment": "Generated by `cargo run -p dropset-interface --example gen_simulate_swap`. Do not edit by hand. `market_data` is the primary market account's raw bytes (incl. the 8-byte discriminator) — two active vaults whose levels interleave in price, so a fill walks across vaults and the cross-vault price-time sort is pinned rather than assumed. `markets` holds the extra fixture buffers in the same format, and each case's `market` field names which buffer it quotes against: \"primary\" means `market_data`, otherwise it is a key in `markets`. The `far_out` market rests an absurdly-priced level behind an honest one on EACH side, pinning that such a level ends the walk and takes nothing rather than absorbing the taker's unspent budget; the `flush` market has its vault's flush bit armed, so its levels are materialized from a relative LiquidityProfile instead of read from `remaining`. Each case lists a swap input (side 0=buy/1=sell, amount_in, limit_price_bits, now_slot, now_unix, platform_fee_bps) and the Quote the native matcher returns. `books` carries the expected resting book — the four parallel arrays wasm::resting_book returns — per market at one clock, and is the ONLY place the cross-vault ordering is asserted directly: a Quote cannot see it, since two levels at one price fill to the same totals in either order, whereas here the sizes ride in the same order as the prices, so an inverted nonce tie-break reorders the size array. Sizes are base-denominated on both sides. Level expiry is dual-domain: a level rests only while it is inside BOTH its slot deadline and its wall-clock deadline, so the `expiry_*` cases pin each bound independently plus the boundary in each domain. A case whose platform_fee_bps exceeds the market's max_platform_fee expects an all-zero Quote: the engine rejects that swap, so the simulator refuses to quote it rather than clamping the rate. Verified against the WASM binding in sdk/interface/tests/wasm_conformance.rs (wasm::simulate_swap == native matcher); the native matcher is pinned to the on-chain engine by programs/dropset/tests/sdk_conformance.rs.",
        "market_data": data,
        "markets": {
            FAR_OUT: far_out_data,
            FLUSH: flush_data,
        },
        "cases": cases,
        "books": books,
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
