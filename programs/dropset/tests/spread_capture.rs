// cspell:word signum
// cspell:word xorshift
//! Spread capture end to end — a vault quoting a passive bell-shaped
//! ladder of reverse orders earns under ordinary price noise. A taker
//! arbitrages the book toward a hidden true price that random-walks about
//! a constant nominal; every round trip across a level rests its proceeds
//! back at the level's flip price, so the vault's inventory, valued at the
//! nominal, grows by the flip spread. Against the deployed `.so` on the
//! shared [`Fixture`].
//!
//! The capture is split two ways. **Fee** is the flip-spread slice — on a
//! fill against a flipped entry, the gap between the level's own price and
//! its flip price, the reverse-order analogue of an AMM's LP fee. **Spread**
//! is the remainder of the change in value at the nominal: the ladder's
//! offset edge on inventory still open. The market's taker fee is not part
//! of either — it is carved from the taker's output into the market's
//! accrued-fee counters, never credited to the vault — and is reported
//! alongside for completeness.

mod common;

use std::collections::BTreeSet;

use anchor_v2_testing::Keypair;
use common::fixture::{flip_profile, Fixture, PROFILE_BYTES};
use dropset::{FillEvent, Price, Vault, N_LEVELS};

const SEED_BASE: u64 = 1_000_000_000;
const SEED_QUOTE: u64 = 1_085_000_000;
const BUY: u8 = 0;
const SELL: u8 = 1;
const PPM: i64 = 1_000_000;
/// The fixture's reference significand: 1.0850 at exponent 0.
const NOMINAL_SIG: i64 = 10_850_000;
/// Level spacing: level `i` sits `(i + 1) × SPACING` ppm off the nominal.
const SPACING: u32 = 1_000;
/// The flip spread every level quotes, in ppm — under the spacing, so a
/// flipped entry rests between its own level and the nominal.
const FLIP_SPREAD: u32 = 600;
/// Per-level share of each leg, largest at the nominal and tapering
/// outward: one half of a bell, mirrored on the other side. Σ = 10000.
const BELL_BPS: [u16; N_LEVELS] = [2_200, 1_900, 1_500, 1_300, 1_100, 900, 600, 500];
/// Entries a single take may cross. A take buffers one `FillEvent` per leg
/// in the bump heap, which holds about ten (`flip.rs` measures it), so the
/// taker clips its limit at the sixth entry and leaves headroom for ties.
const MAX_LEGS: usize = 6;
/// A non-zero market taker fee, so the run shows it landing in the
/// market's accrued counters and nowhere in the vault's capture.
const TAKER_FEE_PPM: u16 = 300;

fn bell_ladder() -> [u8; PROFILE_BYTES] {
    let levels: Vec<(u32, u16, u32)> = (0..N_LEVELS)
        .map(|i| ((i as u32 + 1) * SPACING, BELL_BPS[i], u32::MAX))
        .collect();
    flip_profile(&levels, &levels, FLIP_SPREAD)
}

/// The true price `delta_ppm` off the nominal.
fn price_at(delta_ppm: i64) -> Price {
    Price::encode((NOMINAL_SIG * (PPM + delta_ppm) / PPM) as u32, 0).unwrap()
}

/// Inventory valued in quote atoms at `p`.
fn value_at(p: Price, base: u64, quote: u64) -> i128 {
    quote as i128 + p.quote_for_base(base) as i128
}

/// A seeded xorshift, so a regression replays the same walk.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// Uniform in `[lo, hi]`.
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }

    /// One take in three spends only 20–80% of what the book through the
    /// price would take, as a percentage; the rest take it all.
    fn clip(&mut self) -> Option<u64> {
        self.next()
            .is_multiple_of(3)
            .then(|| self.range(20, 80) as u64)
    }
}

/// Every entry a take on `side` meets at or through `limit`, best first,
/// as `(price, size)` — size in the leg the vault gives up (base for a
/// Buy, quote for a Sell). Each level contributes its own entry and the flipped
/// entry its opposite level rests.
fn crossing(v: &Vault, side: u8, limit: Price) -> Vec<(Price, u64)> {
    let mut entries = Vec::new();
    for l in 0..N_LEVELS {
        let (own, flipped) = if side == BUY {
            (v.remaining.asks[l], v.remaining.bids[l])
        } else {
            (v.remaining.bids[l], v.remaining.asks[l])
        };
        entries.push((own.price, own.size.get()));
        entries.push((flipped.flip_price, flipped.flip_size.get()));
    }
    entries.retain(|&(p, size)| {
        size > 0 && p.is_matchable() && if side == BUY { p <= limit } else { p >= limit }
    });
    entries.sort_by_key(|&(p, _)| p);
    if side == SELL {
        entries.reverse();
    }
    entries
}

#[derive(Default)]
struct Tally {
    swaps: u64,
    fills: u64,
    partial_takes: u64,
    /// Takes that filled more than one distinct level.
    multi_level_takes: u64,
    /// Takes whose limit [`MAX_LEGS`] pulled in short of the true price.
    capped_takes: u64,
    max_legs: usize,
    /// Fills against a flipped entry, by the side the taker took.
    flipped_buys: u64,
    flipped_sells: u64,
    /// The flip-spread slice, in quote atoms.
    fee: i128,
}

impl Tally {
    /// Book the flip-spread slice of one fill. `v` is the pre-take vault,
    /// whose level prices the take does not move.
    fn book(&mut self, v: &Vault, fill: &FillEvent) {
        let l = fill.level_idx as usize;
        self.fills += 1;
        if fill.side == BUY && fill.fill_price != v.remaining.asks[l].price {
            // A flipped ask: the vault sold base above the bid it came from.
            let own = v.remaining.bids[l].price.quote_for_base(fill.fill_base);
            self.fee += fill.fill_quote as i128 - own as i128;
            self.flipped_buys += 1;
        } else if fill.side == SELL && fill.fill_price != v.remaining.bids[l].price {
            // A flipped bid: the vault bought base below the ask it came from.
            let own = v.remaining.asks[l].price.quote_for_base(fill.fill_base);
            self.fee += own as i128 - fill.fill_quote as i128;
            self.flipped_sells += 1;
        }
    }
}

/// The side whose entries are priced through `truth`, if any. Both can be:
/// a flipped ask and a flipped bid can straddle the price, each resting
/// between its own level and the nominal.
fn crossed_side(v: &Vault, truth: Price) -> Option<u8> {
    [BUY, SELL]
        .into_iter()
        .find(|&side| !crossing(v, side, truth).is_empty())
}

/// Arbitrage the book toward `truth`. On `clip`, one partial take, so the
/// book lags the price and the next step finds a partial level; otherwise
/// take both sides until nothing is priced through it.
fn arbitrage(f: &mut Fixture, taker: &Keypair, truth: Price, clip: Option<u64>, t: &mut Tally) {
    if clip.is_some() {
        if let Some(side) = crossed_side(&f.vault(0), truth) {
            take(f, taker, side, truth, clip, t);
        }
        return;
    }
    // Each take clears up to MAX_LEGS entries, so the bound is generous.
    for _ in 0..4 * N_LEVELS {
        match crossed_side(&f.vault(0), truth) {
            Some(side) => take(f, taker, side, truth, None, t),
            None => return,
        }
    }
    panic!("the book never settled at {truth:?}");
}

/// One take on `side` of every entry priced through `truth`, its limit
/// pulled in to the [`MAX_LEGS`]th entry when more cross, spending on
/// `clip` only that percentage of what the take needs.
fn take(
    f: &mut Fixture,
    taker: &Keypair,
    side: u8,
    truth: Price,
    clip: Option<u64>,
    t: &mut Tally,
) {
    let v = f.vault(0);
    let entries = crossing(&v, side, truth);
    if entries.len() > MAX_LEGS {
        t.capped_takes += 1;
    }
    let taken = &entries[..entries.len().min(MAX_LEGS)];
    let limit = if entries.len() > MAX_LEGS {
        taken[MAX_LEGS - 1].0
    } else {
        truth
    };
    let needed: u64 = taken
        .iter()
        .map(|&(p, size)| {
            let input = if side == BUY {
                p.quote_for_base(size)
            } else {
                p.base_for_quote(size)
            };
            input as u64 + 2
        })
        .sum();
    let amount_in = match clip {
        Some(pct) => (needed * pct / 100).max(1),
        None => needed,
    };
    if clip.is_some() {
        t.partial_takes += 1;
    }
    f.svm.expire_blockhash();
    let meta = f
        .swap_meta(taker, side, amount_in, limit.as_u32(), 0)
        .expect("arbitrage take");
    let fills = common::events::fills(&meta);
    let levels: BTreeSet<u32> = fills.iter().map(|fill| fill.level_idx).collect();
    if levels.len() > 1 {
        t.multi_level_takes += 1;
    }
    t.max_legs = t.max_legs.max(fills.len());
    for fill in &fills {
        t.book(&v, fill);
    }
    t.swaps += 1;
}

struct Outcome {
    tally: Tally,
    /// Change in vault value at the nominal, in quote atoms.
    at_nominal: i128,
    /// Vault value minus the untouched seed's, both at the final price.
    vs_hold: i128,
    final_delta_ppm: i64,
    /// Nothing on either side priced through the final price.
    settled: bool,
}

/// Run one regime: `path` yields the true price's ppm offset from the
/// nominal at each step, and `clip`, given the step's index, decides
/// whether the taker takes only part of the book.
fn run(path: &[i64], mut clip: impl FnMut(usize) -> Option<u64>) -> Outcome {
    let mut f = Fixture::seeded_with(SEED_BASE, SEED_QUOTE, bell_ladder());
    let auth = f.authority.insecure_clone();
    f.set_taker_fee(&auth, TAKER_FEE_PPM).expect("taker fee");
    let taker = f.funded_depositor(20 * SEED_BASE, 20 * SEED_QUOTE);
    // A reverted sell materializes the ladder without filling, so the
    // first step reads real level prices off the book.
    f.swap(&taker, SELL, 1, Price::ZERO.as_u32(), u64::MAX)
        .expect("priming revert");
    let start = f.vault(0);
    let nominal = price_at(0);
    let v0 = value_at(nominal, start.base_atoms.get(), start.quote_atoms.get());

    let mut tally = Tally::default();
    for (i, &delta) in path.iter().enumerate() {
        arbitrage(&mut f, &taker, price_at(delta), clip(i), &mut tally);
    }

    let end = f.vault(0);
    let (base, quote) = (end.base_atoms.get(), end.quote_atoms.get());
    let last = *path.last().unwrap();
    let fin = price_at(last);
    let (residual_base, residual_quote) = f.treasury_residual();
    assert!(
        residual_base <= 2 * tally.swaps && residual_quote <= 2 * tally.swaps,
        "residual ({residual_base}, {residual_quote}) after {} swaps is more than dust",
        tally.swaps
    );
    Outcome {
        at_nominal: value_at(nominal, base, quote) - v0,
        vs_hold: value_at(fin, base, quote) - value_at(fin, SEED_BASE, SEED_QUOTE),
        final_delta_ppm: last,
        settled: crossed_side(&end, fin).is_none(),
        tally,
    }
    .report(&f)
}

impl Outcome {
    fn report(self, f: &Fixture) -> Self {
        let h = f.market_header();
        let t = &self.tally;
        println!(
            "{} swaps, {} fills ({} flipped buys, {} flipped sells, {} clipped takes, \
             {} multi-level takes, {} capped at {MAX_LEGS} entries, at most {} legs); \
             at the nominal: {:+} quote atoms = fee {:+} + spread {:+} \
             ({:+.2} bps of the seed); vs holding at the final price \
             ({:+} ppm): {:+}; taker fee to the market: {} base, {} quote",
            t.swaps,
            t.fills,
            t.flipped_buys,
            t.flipped_sells,
            t.partial_takes,
            t.multi_level_takes,
            t.capped_takes,
            t.max_legs,
            self.at_nominal,
            t.fee,
            self.at_nominal - t.fee,
            self.at_nominal as f64 * 1e4 / (2 * SEED_QUOTE) as f64,
            self.final_delta_ppm,
            self.vs_hold,
            h.accrued_base_fee_atoms.get(),
            h.accrued_quote_fee_atoms.get(),
        );
        self
    }
}

/// Noise about the nominal: steps of up to ±1000 ppm, one in ten a jump of
/// up to ±2500 that sweeps several levels at once, reflected at ±5000 so
/// the walk stays well inside the ladder's 8000 ppm span and keeps crossing
/// back through the nominal. Then walked home taking every crossing entry
/// whole, so the book ends
/// settled at the nominal and what remains open is a residue rather than a
/// position.
#[test]
fn a_random_walk_about_the_nominal_grows_the_vault_by_the_flip_spread() {
    const REFLECT: i64 = 5_000;
    const STEPS: usize = 600;
    let mut rng = Rng(0x5eed_1872);
    let (mut delta, mut crossings) = (0i64, 0);
    let mut path: Vec<i64> = (0..STEPS)
        .map(|_| {
            let reach = if rng.next().is_multiple_of(10) {
                2_500
            } else {
                1_000
            };
            let next = delta + rng.range(-reach, reach);
            let next = if next.abs() > REFLECT {
                2 * REFLECT * next.signum() - next
            } else {
                next
            };
            if next.signum() * delta.signum() < 0 {
                crossings += 1;
            }
            delta = next;
            delta
        })
        .collect();
    while delta != 0 {
        delta -= delta.signum() * delta.abs().min(500);
        path.push(delta);
    }
    assert!(
        crossings >= 10,
        "the walk crossed the nominal {crossings} times"
    );

    let mut clips = Rng(0xc11b);
    let out = run(&path, |i| if i < STEPS { clips.clip() } else { None });
    let t = &out.tally;
    assert!(out.settled, "the walk home left the book crossed");
    assert!(t.multi_level_takes > 0, "no take swept more than one level");
    // Round trips both ways, repeatedly — a flipped fill is the second half
    // of one.
    assert!(
        t.flipped_buys >= 10 && t.flipped_sells >= 10,
        "too few round trips: {} flipped buys, {} flipped sells",
        t.flipped_buys,
        t.flipped_sells
    );
    assert!(t.partial_takes > 0);
    assert!(t.fee > 0, "the flip spread earned nothing");
    assert!(out.at_nominal > 0, "the vault did not grow at the nominal");
    // Walked home, the offset edge has round-tripped away: what the vault
    // keeps is the flip spread, to within a second-order residue (each
    // return leg is a little larger than the leg it reverses).
    let spread = out.at_nominal - t.fee;
    assert!(
        spread.abs() * 100 < t.fee,
        "spread {spread} is not a residue of the fee {}",
        t.fee
    );
}

/// The ladder's promise under a one-way drift is not growth. Every own
/// fill trades on the vault's side of the nominal, so its value there can
/// only rise; but nothing comes back to round-trip, so the flip spread
/// earns exactly nothing, and against simply holding at the price the
/// drift reached the vault is behind — the loss to repricing the flip
/// spread exists to pay for, uncompensated when the price never returns.
fn assert_drift(sign: i64) {
    let mut rng = Rng(if sign > 0 { 0xd41f7 } else { 0x7f14d });
    let mut delta = 0i64;
    let path: Vec<i64> = std::iter::from_fn(|| {
        delta += sign * rng.range(100, 900);
        (delta.abs() <= 9_000).then_some(delta)
    })
    .collect();
    let mut clips = Rng(0xc11b);
    let out = run(&path, |_| clips.clip());
    let t = &out.tally;
    assert!(t.fills > 0);
    assert_eq!(
        (t.flipped_buys, t.flipped_sells),
        (0, 0),
        "a drift never round-trips"
    );
    assert_eq!(t.fee, 0);
    assert!(out.at_nominal > 0, "own fills trade on the vault's side");
    assert!(out.vs_hold < 0, "a drifted-through ladder trails holding");
}

#[test]
fn a_drift_up_earns_no_fee_and_trails_holding() {
    assert_drift(1);
}

#[test]
fn a_drift_down_earns_no_fee_and_trails_holding() {
    assert_drift(-1);
}

/// A price holding still — noise that never reaches the innermost level,
/// 1000 ppm out — crosses no entry: no take, no fill, and the vault exactly
/// where it started.
#[test]
fn a_still_price_trades_nothing() {
    let mut rng = Rng(0x5711);
    let path: Vec<i64> = (0..40).map(|_| rng.range(-900, 900)).collect();
    let out = run(&path, |_| None);
    assert_eq!(out.tally.swaps, 0);
    assert_eq!(out.at_nominal, 0);
    assert_eq!(out.vs_hold, 0);
}
