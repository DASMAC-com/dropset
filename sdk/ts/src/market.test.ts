/**
 * Unit-test the market slab decode + level materialization against a
 * hand-built fixture — the TS counterpart to the Rust `matching.rs` tests.
 * The fixture mirrors those: a one-vault EUR/USD market, exercised on both
 * the stored `remaining` path and the flush `LiquidityProfile` path.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';

import { decodeMarketSlab, marketViewFromSlab } from './market';
import { baseForQuote, priceFromParts } from './price';

// Slab layout constants (mirror `layout.rs`): discriminator + on-chain
// MarketHeader (237) = the offset of the u32 slab length; the first sector
// starts at the next 4-byte boundary after it.
const DISCRIMINATOR = 8;
const HEADER = 237;
const LEN_AT = DISCRIMINATOR + HEADER; // 245
const ITEMS_START = (LEN_AT + 4 + 3) & ~3; // 252
const VAULT = 560;

// MarketHeader field offsets, relative to the start of the header (i.e.
// after the discriminator). Only `head` / `activeCount` matter to the reader.
const H_HEAD = 8;
const H_ACTIVE_COUNT = 20;

// Vault field offsets (relative to the sector start).
const V_REF_STAMP = 72;
const V_REF_PRICE = 80;
const V_REF_QUOTE_SLOT = 84;
const V_BASE_ATOMS = 88;
const V_QUOTE_ATOMS = 96;
const V_PROFILE_BIDS = 144;
const V_PROFILE_ASKS = 224;
const V_REMAINING_BIDS = 304;
const V_REMAINING_ASKS = 432;

const NULL_SECTOR = 0xffff_ffff;

/** Raw `Price` bits for a significand at unbiased exponent 0 (biased 16). */
const enc = (significand: number) => priceFromParts(significand, 16);

function writePosition(
  dv: DataView,
  offset: number,
  price: number,
  size: bigint,
  expiresAt = NULL_SECTOR,
): void {
  dv.setUint32(offset, price, true);
  dv.setBigUint64(offset + 4, size, true);
  dv.setUint32(offset + 12, expiresAt, true);
}

function writeProfileLevel(
  dv: DataView,
  offset: number,
  priceOffset: number,
  sizeBps: number,
  expiryOffset: number,
): void {
  dv.setUint32(offset, priceOffset, true);
  dv.setUint16(offset + 4, sizeBps, true);
  dv.setUint32(offset + 6, expiryOffset, true);
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
    dv.setBigUint64(b + V_BASE_ATOMS, 10_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 10_000_000n, true);
    writePosition(dv, b + V_REMAINING_ASKS, enc(10_904_000), 1_000_000n);
    writePosition(dv, b + V_REMAINING_ASKS + 16, enc(11_393_000), 800_000n);
    writePosition(dv, b + V_REMAINING_BIDS, enc(10_796_000), 2_000_000n);
    writePosition(dv, b + V_REMAINING_BIDS + 16, enc(10_416_000), 1_500_000n);
  });
}

test('remaining asks are best-first and base-sized', () => {
  const view = marketViewFromSlab(decodeMarketSlab(remainingMarket()), 1);
  assert.deepEqual(view.asks, [
    { price: enc(10_904_000), size: 1_000_000n },
    { price: enc(11_393_000), size: 800_000n },
  ]);
});

test('remaining bids are best-first and normalized to base', () => {
  const view = marketViewFromSlab(decodeMarketSlab(remainingMarket()), 1);
  const best = enc(10_796_000);
  const next = enc(10_416_000);
  assert.deepEqual(view.bids, [
    { price: best, size: baseForQuote(best, 2_000_000n) },
    { price: next, size: baseForQuote(next, 1_500_000n) },
  ]);
});

test('levels expired at the current slot are excluded', () => {
  // Every level expires at u32::MAX; past it the book is empty both sides.
  const view = marketViewFromSlab(decodeMarketSlab(remainingMarket()), NULL_SECTOR);
  assert.equal(view.asks.length, 0);
  assert.equal(view.bids.length, 0);
});

test('flush-armed vault materializes levels from its profile', () => {
  // EUR/USD 1.0850, ±500 ppm, full-leg (10000 bps) on the top rung.
  const data = buildMarket((dv, b) => {
    dv.setBigUint64(b + V_REF_STAMP, (1n << 63n) | 1n, true); // flush armed, nonce 1
    dv.setUint32(b + V_REF_PRICE, enc(10_850_000), true);
    dv.setUint32(b + V_REF_QUOTE_SLOT, 0, true);
    dv.setBigUint64(b + V_BASE_ATOMS, 1_000_000n, true);
    dv.setBigUint64(b + V_QUOTE_ATOMS, 1_000_000n, true);
    writeProfileLevel(dv, b + V_PROFILE_ASKS, 500, 10_000, 1_000);
    writeProfileLevel(dv, b + V_PROFILE_BIDS, 500, 10_000, 1_000);
  });
  const view = marketViewFromSlab(decodeMarketSlab(data), 1);
  // ref × (1e6 ± 500)/1e6: 10_850_000 → 10_855_425 (ask) / 10_844_575 (bid).
  assert.deepEqual(view.asks, [{ price: enc(10_855_425), size: 1_000_000n }]);
  const bidPrice = enc(10_844_575);
  assert.deepEqual(view.bids, [{ price: bidPrice, size: baseForQuote(bidPrice, 1_000_000n) }]);
});
