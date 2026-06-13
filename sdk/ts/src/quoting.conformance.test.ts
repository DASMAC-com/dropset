/**
 * Verify the TS quoting fork against the shared quoting vectors — the same
 * `sdk/conformance/quoting_vectors.json` the Rust SDK checks
 * (sdk/rs/tests/quoting_conformance.rs). This pins the hand-written
 * native-book → relative-profile translation in `quoting.ts` to the one
 * reference encoded in the generator
 * (sdk/price-core/examples/gen_quoting.rs) — ENG-476 hole 2.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { N_LEVELS, nativeBookToProfileBytes, type NativeBook, type NativeLevel } from './quoting';

type LevelCase = {
  price_bits: number;
  size: number;
  expiry_offset: number;
  price_offset: number;
  size_bps: number;
};
type Case = {
  reference_bits: number;
  base_atoms: number;
  quote_atoms: number;
  asks: LevelCase[];
  bids: LevelCase[];
};

const vectors = JSON.parse(
  readFileSync(new URL('../../conformance/quoting_vectors.json', import.meta.url), 'utf8'),
) as { cases: Case[] };

const LEVEL_BYTES = 10;

function nativeLevel(c: LevelCase): NativeLevel {
  return { price: c.price_bits, size: BigInt(c.size), expiryOffset: c.expiry_offset };
}

/** Decode one serialized relative level: u32 offset, u16 bps, u32 expiry (LE). */
function readLevel(view: DataView, index: number) {
  const o = index * LEVEL_BYTES;
  return {
    priceOffset: view.getUint32(o, true),
    sizeBps: view.getUint16(o + 4, true),
    expiryOffset: view.getUint32(o + 6, true),
  };
}

test('quoting vectors match', () => {
  for (const c of vectors.cases) {
    const book: NativeBook = {
      asks: c.asks.map(nativeLevel),
      bids: c.bids.map(nativeLevel),
    };
    const bytes = nativeBookToProfileBytes(
      book,
      c.reference_bits,
      BigInt(c.base_atoms),
      BigInt(c.quote_atoms),
    );
    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);

    // Layout: bids[0..N_LEVELS] then asks[0..N_LEVELS].
    c.bids.forEach((exp, i) => {
      const got = readLevel(view, i);
      assert.equal(got.priceOffset, exp.price_offset, `bid[${i}] offset`);
      assert.equal(got.sizeBps, exp.size_bps, `bid[${i}] size_bps`);
      assert.equal(got.expiryOffset, exp.expiry_offset, `bid[${i}] expiry`);
    });
    c.asks.forEach((exp, i) => {
      const got = readLevel(view, N_LEVELS + i);
      assert.equal(got.priceOffset, exp.price_offset, `ask[${i}] offset`);
      assert.equal(got.sizeBps, exp.size_bps, `ask[${i}] size_bps`);
      assert.equal(got.expiryOffset, exp.expiry_offset, `ask[${i}] expiry`);
    });
  }
});
