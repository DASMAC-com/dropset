/**
 * Unit-test the market reader against a hand-built fixture — the TS
 * counterpart to the Rust `matching.rs` tests. The fixture mirrors those: a
 * one-vault EUR/USD market, exercised on both the stored `remaining` path
 * and the flush `LiquidityProfile` path.
 *
 * **What this file pins, now that the reader delegates to the engine.** The
 * decode and the book math live in Rust and reach TS through the WASM
 * binding, so these tests are not re-testing the materialization rules —
 * `matching.rs` owns those, and its own tests cover them. What they test is
 * the seam: that the byte layout written below is the byte layout the engine
 * reads, and that both sides marshal back across the boundary intact.
 *
 * That makes the offset constants here load-bearing in a way they were not
 * before. They used to be a second copy of the same constants the reader
 * itself declared, so the two agreed with each other whether or not either
 * agreed with the chain — which is exactly how the layout drifted once. Now
 * the fixture is the input and the engine is the oracle: if `layout.rs`
 * moves a field and this file doesn't follow, these tests go red instead of
 * quietly agreeing with a stale mirror.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { getAddressDecoder } from '@solana/kit';

import { slotTime, wallTime } from './clock';
import { decodeDropsetMarketView, decodeDropsetVaults } from './market';
import { baseForQuote, priceFromParts } from './price';
import { initSimulator } from './simulate';

// `initSimulator` resolves the `.wasm` asset relative to the module by
// default, which needs a bundler's asset pipeline; under Node we hand it the
// committed bytes. The market reader and the swap simulator share one module
// instance, so this single init covers both.
await initSimulator(readFileSync(new URL('./wasm/dropset_interface_bg.wasm', import.meta.url)));

// Slab layout constants (mirror `layout.rs`): discriminator + on-chain
// MarketHeader (396) = the offset of the u32 slab length; the first sector
// starts at the next 4-byte boundary after it, which it already is.
const DISCRIMINATOR = 8;
const HEADER = 396;
const LEN_AT = DISCRIMINATOR + HEADER; // 404
const ITEMS_START = (LEN_AT + 4 + 3) & ~3; // 408
const VAULT = 1088;

// MarketHeader field offsets, relative to the start of the header (i.e.
// after the discriminator). Only `head` / `activeCount` matter to the reader.
const H_HEAD = 240;
const H_ACTIVE_COUNT = 252;

// Vault field offsets (relative to the sector start).
const V_REF_STAMP = 112;
const V_REF_PRICE = 120;
const V_REF_QUOTE_SLOT = 124;
const V_REF_QUOTE_UNIX = 128;
const V_BASE_ATOMS = 132;
const V_QUOTE_ATOMS = 140;
// Level is 18 bytes and Position 32, so each side's array is 144 / 256.
const V_PROFILE_BIDS = 288;
const V_PROFILE_ASKS = 432;
const V_REMAINING_BIDS = 576;
const V_REMAINING_ASKS = 832;
/** `Level` / `Position` strides — 18 and 32 bytes with the flip fields. */
const LEVEL = 18;
const POSITION = 32;

const NULL_SECTOR = 0xffff_ffff;

/** Raw `Price` bits for a significand at unbiased exponent 0 (biased 16). */
const enc = (significand: number) => priceFromParts(significand, 16);

function writePosition(
  dv: DataView,
  offset: number,
  price: number,
  size: bigint,
  expiresAtUnix = NULL_SECTOR,
  expiresAtSlot = NULL_SECTOR,
): void {
  dv.setUint32(offset, price, true);
  dv.setBigUint64(offset + 4, size, true);
  dv.setUint32(offset + 12, expiresAtUnix, true);
  dv.setUint32(offset + 16, expiresAtSlot, true);
}

/** Write a `Position`'s flipped counter: `flip_price` at `+ 20`, `flip_size`
 * (atoms on the leg opposite `size`) at `+ 24`. */
function writeFlip(dv: DataView, offset: number, flipPrice: number, flipSize: bigint): void {
  dv.setUint32(offset + 20, flipPrice, true);
  dv.setBigUint64(offset + 24, flipSize, true);
}

function writeProfileLevel(
  dv: DataView,
  offset: number,
  priceOffset: number,
  sizeBps: number,
  expiryOffsetSecs: number,
  expiryOffsetSlots = NULL_SECTOR,
): void {
  dv.setUint32(offset, priceOffset, true);
  // offset + 4: `flip_spread_ppm`, left zero (the level does not flip).
  dv.setUint16(offset + 8, sizeBps, true);
  dv.setUint32(offset + 10, expiryOffsetSecs, true);
  dv.setUint32(offset + 14, expiryOffsetSlots, true);
}

/** A one-vault, single-sector market. `configure` fills the vault sector. */
function buildMarket(configure: (dv: DataView, vaultBase: number) => void): Uint8Array {
  const buf = new Uint8Array(ITEMS_START + VAULT);
  const dv = new DataView(buf.buffer);
  dv.setUint32(DISCRIMINATOR + H_HEAD, 0, true); // active DLL head = sector 0
  dv.setUint32(DISCRIMINATOR + H_ACTIVE_COUNT, 1, true);
  dv.setUint32(LEN_AT, 1, true); // slab length = 1 sector
  const vaultBase = ITEMS_START;
  dv.setUint32(vaultBase, NULL_SECTOR, true); // vault.next = NULL (single node)
  configure(dv, vaultBase);
  return buf;
}

/**
 * A vault carrying a live EUR/USD book in its `remaining` positions (no
 * flush armed): two asks and two bids, same shape as the Rust fixture.
 */
function remainingMarket(): Uint8Array {
  return buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, 1n, true); // stamp = 1, flush bit clear
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setUint32(b + V_REF_QUOTE_SLOT, 0, true);
    dv.setUint32(b + V_REF_QUOTE_UNIX, 0, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 10_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 10_000_000n, true);
    writePosition(dv, b + V_REMAINING_ASKS, enc(10_904_000), 1_000_000n);
    writePosition(dv, b + V_REMAINING_ASKS + POSITION, enc(11_393_000), 800_000n);
    writePosition(dv, b + V_REMAINING_BIDS, enc(10_796_000), 2_000_000n);
    writePosition(dv, b + V_REMAINING_BIDS + POSITION, enc(10_416_000), 1_500_000n);
  });
}

test('remaining asks are best-first and base-sized', () => {
  const view = decodeDropsetMarketView(remainingMarket(), slotTime(1), wallTime(2));
  assert.deepEqual(view.asks, [
    { price: enc(10_904_000), size: 1_000_000n },
    { price: enc(11_393_000), size: 800_000n },
  ]);
});

test('remaining bids are best-first and normalized to base', () => {
  const view = decodeDropsetMarketView(remainingMarket(), slotTime(1), wallTime(2));
  const best = enc(10_796_000);
  const next = enc(10_416_000);
  assert.deepEqual(view.bids, [
    { price: best, size: baseForQuote(best, 2_000_000n) },
    { price: next, size: baseForQuote(next, 1_500_000n) },
  ]);
});

// A flipped counter is not a new kind of level: the engine hands it back as
// an ordinary entry on the opposite side, at its `flip_price`, sorted into
// price-time priority with every own level. These two pin that it crosses the
// seam that way — the `flip_*` offsets above are as load-bearing as the rest.

test('a partially flipped bid rests on both sides', () => {
  // `bids[0]` has been part-filled: 1_000_000 quote atoms still bid, and the
  // 400_000 base atoms it bought rest as an ask at its flip price, inside the
  // vault's own best ask.
  const bid = enc(10_796_000);
  const flipAsk = enc(10_801_000);
  const data = buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, 1n, true); // flush bit clear
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setBigUint64(b + V_BASE_ATOMS, 10_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 10_000_000n, true);
    writePosition(dv, b + V_REMAINING_ASKS, enc(10_904_000), 1_000_000n);
    writePosition(dv, b + V_REMAINING_BIDS, bid, 1_000_000n);
    writeFlip(dv, b + V_REMAINING_BIDS, flipAsk, 400_000n);
  });
  const view = decodeDropsetMarketView(data, slotTime(1), wallTime(2));
  assert.deepEqual(view.asks, [
    { price: flipAsk, size: 400_000n },
    { price: enc(10_904_000), size: 1_000_000n },
  ]);
  assert.deepEqual(view.bids, [{ price: bid, size: baseForQuote(bid, 1_000_000n) }]);
});

test("a filled ask's flipped bid is base-sized and ties after the own level", () => {
  // `asks[0]` is filled out (size 0), and its quote proceeds rest as a bid at
  // exactly the price `bids[0]` already quotes. The engine reports the two
  // separately — the own counter first — and each is converted to base at the
  // shared price; folding them into one row is the view's job, not the book's.
  const shared = enc(10_796_000);
  const data = buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, 1n, true); // flush bit clear
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setBigUint64(b + V_BASE_ATOMS, 10_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 10_000_000n, true);
    writePosition(dv, b + V_REMAINING_ASKS, enc(10_904_000), 0n);
    writeFlip(dv, b + V_REMAINING_ASKS, shared, 1_085_000n);
    writePosition(dv, b + V_REMAINING_BIDS, shared, 2_000_000n);
  });
  const view = decodeDropsetMarketView(data, slotTime(1), wallTime(2));
  assert.deepEqual(view.asks, [], 'a filled-out ask rests nothing, and no bid flipped onto this side');
  assert.deepEqual(view.bids, [
    { price: shared, size: baseForQuote(shared, 2_000_000n) },
    { price: shared, size: baseForQuote(shared, 1_085_000n) },
  ]);
});

test('the header decodes alongside the book', () => {
  // The header comes from the IDL-generated codec and the book from the
  // engine; this pins that the two agree on where the header ends, which is
  // the one offset the TS side still has to know.
  const view = decodeDropsetMarketView(remainingMarket(), slotTime(1), wallTime(2));
  assert.equal(view.header.head, 0);
  assert.equal(view.header.activeCount, 1);
});

test('levels expired at the current slot are excluded', () => {
  // Every level expires at u32::MAX; past it the book is empty both sides.
  const view = decodeDropsetMarketView(
    remainingMarket(),
    slotTime(NULL_SECTOR),
    wallTime(NULL_SECTOR),
  );
  assert.equal(view.asks.length, 0);
  assert.equal(view.bids.length, 0);
});

test('flush-armed vault materializes levels from its profile', () => {
  // EUR/USD 1.0850, ±500 ppm, full-leg (10000 bps) on the top rung.
  const data = buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, (1n << 63n) | 1n, true); // flush armed, nonce 1
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setUint32(b + V_REF_QUOTE_SLOT, 0, true);
    dv.setUint32(b + V_REF_QUOTE_UNIX, 0, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 1_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 1_000_000n, true);
    writeProfileLevel(dv, b + V_PROFILE_ASKS, 500, 10_000, 1_000);
    writeProfileLevel(dv, b + V_PROFILE_BIDS, 500, 10_000, 1_000);
  });
  const view = decodeDropsetMarketView(data, slotTime(1), wallTime(2));
  // ref × (1e6 ± 500)/1e6: 10_850_000 → 10_855_425 (ask) / 10_844_575 (bid).
  assert.deepEqual(view.asks, [{ price: enc(10_855_425), size: 1_000_000n }]);
  const bidPrice = enc(10_844_575);
  assert.deepEqual(view.bids, [{ price: bidPrice, size: baseForQuote(bidPrice, 1_000_000n) }]);
});

// The two datums, deliberately far apart, so a transposition of the two
// expiry fields moves a materialized deadline by ~1.7e9 rather than by a few
// units. The other fixtures set both to 0, which would let a swap pass
// unnoticed — both are plain `number`s, so nothing else would catch it.
const FIX_QUOTE_SLOT = 7;
const FIX_QUOTE_UNIX = 1_700_000_000;
/** Absolute deadlines for the stored-`remaining` fixture, far apart so a
 * transposed offset moves one by ~1.7e9. */
const SLOT_DEADLINE = 1_000;
const WALL_DEADLINE = 1_700_000_600;

/** A flush-armed vault with finite, per-domain expiry offsets. */
function boundedFlushMarket(secs: number, slots: number): Uint8Array {
  return buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, (1n << 63n) | 1n, true); // flush armed
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setUint32(b + V_REF_QUOTE_SLOT, FIX_QUOTE_SLOT, true);
    dv.setUint32(b + V_REF_QUOTE_UNIX, FIX_QUOTE_UNIX, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 1_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 1_000_000n, true);
    writeProfileLevel(dv, b + V_PROFILE_ASKS, 500, 10_000, secs, slots);
    writeProfileLevel(dv, b + V_PROFILE_BIDS, 500, 10_000, secs, slots);
  });
}

/** A `remaining`-path vault (no flush) with distinct finite deadlines. */
function boundedRemainingMarket(): Uint8Array {
  return buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, 1n, true); // flush bit clear
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setBigUint64(b + V_BASE_ATOMS, 10_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 10_000_000n, true);
    writePosition(
      dv,
      b + V_REMAINING_ASKS,
      enc(10_904_000),
      1_000_000n,
      WALL_DEADLINE,
      SLOT_DEADLINE,
    );
  });
}

// The stored-`remaining` path writes its two deadlines to two adjacent u32
// fields (`o + 12` / `o + 16`). Every other non-flush fixture writes both to
// the same value, so transposing them here would leave the suite green.
// Distinct deadlines make a swapped pair — in the fixture or in `layout.rs`
// — a red test.
test('the stored remaining path reads each deadline from its own field', () => {
  const data = boundedRemainingMarket();
  assert.equal(
    decodeDropsetMarketView(data, slotTime(SLOT_DEADLINE - 1), wallTime(WALL_DEADLINE - 1)).asks
      .length,
    1,
    'inside both stored deadlines the level rests',
  );
  assert.equal(
    decodeDropsetMarketView(data, slotTime(SLOT_DEADLINE), wallTime(WALL_DEADLINE - 1)).asks.length,
    0,
    'the stored slot deadline must kill it on its own',
  );
  assert.equal(
    decodeDropsetMarketView(data, slotTime(SLOT_DEADLINE - 1), wallTime(WALL_DEADLINE)).asks.length,
    0,
    'the stored wall deadline must kill it on its own',
  );
});

// Mirrors the Rust `each_expiry_conjunct_is_independently_live`, across the
// boundary: it pins that both clocks reach the engine in the right argument
// slots, which a transposed `now_slot` / `now_unix` at the call site would
// otherwise hide.
test('each expiry conjunct kills a level on its own', () => {
  const SECS_OFF = 600;
  const SLOT_OFF = 50;
  const wallDeadline = FIX_QUOTE_UNIX + SECS_OFF;
  const slotDeadline = FIX_QUOTE_SLOT + SLOT_OFF;
  const data = boundedFlushMarket(SECS_OFF, SLOT_OFF);

  // Inside both bounds.
  assert.equal(
    decodeDropsetMarketView(data, slotTime(slotDeadline - 1), wallTime(wallDeadline - 1)).asks
      .length,
    1,
    'a level inside both deadlines must rest',
  );
  // Slot bound passed, wall bound still open.
  assert.equal(
    decodeDropsetMarketView(data, slotTime(slotDeadline), wallTime(wallDeadline - 1)).asks.length,
    0,
    'the slot conjunct must kill the level on its own',
  );
  // Wall bound passed, slot bound still open.
  assert.equal(
    decodeDropsetMarketView(data, slotTime(slotDeadline - 1), wallTime(wallDeadline)).asks.length,
    0,
    'the wall conjunct must kill the level on its own',
  );
});

test('a zero offset is dead in either domain, whatever the datum', () => {
  // Zero slot offset against a live wall TIF, then the mirror. Materialization
  // encodes zero as the dead sentinel rather than letting the datum stand in.
  assert.equal(
    decodeDropsetMarketView(boundedFlushMarket(600, 0), slotTime(1), wallTime(FIX_QUOTE_UNIX)).asks
      .length,
    0,
    'a zero slot offset never rests, however long the wall TIF',
  );
  assert.equal(
    decodeDropsetMarketView(boundedFlushMarket(0, 600), slotTime(FIX_QUOTE_SLOT), wallTime(2)).asks
      .length,
    0,
    'a zero wall offset never rests, however long the slot bound',
  );
});

test('oversized flush side is skipped, the healthy side still materializes', () => {
  // Ask side sums past BPS (20000 bps), bid side healthy (5000). Mirrors the
  // Rust `oversize_flush_side_is_skipped_not_the_whole_book`: the oversized
  // side is thrown out of matching (like the engine zeroing its `remaining`),
  // while the healthy side still reconstructs — not a whole-book refusal.
  const data = buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, (1n << 63n) | 1n, true); // flush armed
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setUint32(b + V_REF_QUOTE_SLOT, 0, true);
    dv.setUint32(b + V_REF_QUOTE_UNIX, 0, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 1_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 1_000_000n, true);
    writeProfileLevel(dv, b + V_PROFILE_ASKS, 500, 20_000, 1_000); // Σ ask = 20000 > BPS
    writeProfileLevel(dv, b + V_PROFILE_BIDS, 500, 5_000, 1_000); // Σ bid = 5000, healthy
  });
  const view = decodeDropsetMarketView(data, slotTime(1), wallTime(2));
  assert.equal(view.asks.length, 0, 'oversized ask side contributes no depth');
  const bidPrice = enc(10_844_575);
  assert.deepEqual(view.bids, [{ price: bidPrice, size: baseForQuote(bidPrice, 500_000n) }]);
});

// Vault share / flag offsets for the vaults reader (`layout.rs` order:
// the 32-byte name at 8, leader at 40, `seq` at 104 — each pinned there by
// a const assert — then the reserves, shares, hwm, perf fee, leader floor
// and the flag bytes).
const V_LEADER = 40;
const V_SEQ = 104;
const V_TOTAL_SHARES = 148;
const V_LEADER_SHARES = 156;
const V_HWM = 164;
const V_PERF_FEE_RATE = 172;
const V_MIN_LEADER_SHARE = 176;
const V_FROZEN = 180;
const V_ALLOW_OUTSIDE = 181;
const V_OUTSIDE_APPROVED = 182;

test('the vaults reader returns each active vault with its shares and deposit flags', () => {
  const data = buildMarket((dv, b) => {
    for (let i = 0; i < 32; i++) dv.setUint8(b + V_LEADER + i, 7);
    dv.setBigUint64(b + V_SEQ, 3n, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 1_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 1_250_000n, true);
    dv.setBigUint64(b + V_TOTAL_SHARES, 900_000n, true);
    dv.setBigUint64(b + V_LEADER_SHARES, 600_000n, true);
    dv.setBigUint64(b + V_HWM, 5n << 32n, true);
    dv.setUint32(b + V_PERF_FEE_RATE, 200_000, true);
    dv.setUint32(b + V_MIN_LEADER_SHARE, 100_000, true);
    dv.setUint8(b + V_ALLOW_OUTSIDE, 1);
  });
  const [vault, ...rest] = decodeDropsetVaults(data);
  assert.equal(rest.length, 0);
  assert.deepEqual(vault, {
    index: 0,
    seq: 3n,
    leader: getAddressDecoder().decode(new Uint8Array(32).fill(7)),
    baseAtoms: 1_000_000n,
    quoteAtoms: 1_250_000n,
    totalShares: 900_000n,
    leaderShares: 600_000n,
    hwm: 5n << 32n,
    perfFeeRatePpm: 200_000,
    minLeaderSharePpm: 100_000,
    frozen: false,
    allowOutsideDepositors: true,
    outsideDepositsApproved: false,
  });

  // The flags are independent bytes: flip one at a time, so a swapped pair
  // of flags cannot pass.
  const dv = new DataView(data.buffer);
  dv.setUint8(ITEMS_START + V_FROZEN, 1);
  const [frozen] = decodeDropsetVaults(data);
  assert.equal(frozen!.frozen, true);
  assert.equal(frozen!.outsideDepositsApproved, false);
  dv.setUint8(ITEMS_START + V_FROZEN, 0);
  dv.setUint8(ITEMS_START + V_OUTSIDE_APPROVED, 1);
  const [approved] = decodeDropsetVaults(data);
  assert.equal(approved!.frozen, false);
  assert.equal(approved!.outsideDepositsApproved, true);
});

test('the vaults reader follows the active list and keeps each vault aligned', () => {
  // Three sectors, active list 2 → 0, sector 1 unlinked: each vault's index
  // is its sector, not its list position, and the per-vault arrays line up.
  const buf = new Uint8Array(ITEMS_START + 3 * VAULT);
  const dv = new DataView(buf.buffer);
  dv.setUint32(DISCRIMINATOR + H_HEAD, 2, true);
  dv.setUint32(DISCRIMINATOR + H_ACTIVE_COUNT, 2, true);
  dv.setUint32(LEN_AT, 3, true);
  const sector = (i: number) => ITEMS_START + i * VAULT;
  dv.setUint32(sector(2), 0, true); // 2.next = 0
  dv.setUint32(sector(0), NULL_SECTOR, true); // 0.next = NULL
  dv.setUint32(sector(1), NULL_SECTOR, true);
  const leader = (seed: number) => Uint8Array.from({ length: 32 }, (_, k) => seed + k);
  buf.set(leader(1), sector(2) + V_LEADER);
  buf.set(leader(100), sector(0) + V_LEADER);
  dv.setBigUint64(sector(2) + V_SEQ, 7n, true);
  dv.setBigUint64(sector(0) + V_SEQ, 3n, true);
  dv.setBigUint64(sector(2) + V_TOTAL_SHARES, 20n, true);
  dv.setBigUint64(sector(0) + V_TOTAL_SHARES, 10n, true);
  dv.setUint8(sector(0) + V_ALLOW_OUTSIDE, 1);

  const vaults = decodeDropsetVaults(buf);
  const decode = getAddressDecoder();
  assert.deepEqual(
    vaults.map((v) => [v.index, v.seq, v.leader, v.totalShares, v.allowOutsideDepositors]),
    [
      [2, 7n, decode.decode(leader(1)), 20n, false],
      [0, 3n, decode.decode(leader(100)), 10n, true],
    ],
  );
});

test('the vaults reader refuses a corrupt active list', () => {
  // `next` points past the one-sector slab: the engine aborts over this, so
  // the reader must not hand back a truncated list.
  const data = buildMarket((dv, b) => dv.setUint32(b, 5, true));
  assert.throws(() => decodeDropsetVaults(data), /corrupt/);
});

test('a buffer too short to hold the header is rejected', () => {
  assert.throws(
    () => decodeDropsetMarketView(new Uint8Array(16), slotTime(1), wallTime(2)),
    /too small/,
    'a truncated account must surface as a layout error',
  );
});
