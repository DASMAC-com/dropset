/**
 * Verify the TS quoting fork against the shared quoting vectors — the same
 * `sdk/conformance/quoting_vectors.json` the Rust SDK checks
 * (sdk/rs/tests/quoting_conformance.rs). This pins the hand-written
 * native-book → relative-profile translation in `quoting.ts` to the one
 * reference encoded in the generator
 * (sdk/math-core/examples/gen_quoting.rs) — ENG-476 hole 2.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import {
  N_LEVELS,
  nativeBookToProfileBytes,
  QuotingError,
  type NativeBook,
  type NativeLevel,
} from './quoting';

type NativeLevelCase = {
  price_bits: number;
  size: number;
  expiry_offset: number;
};
type LevelCase = NativeLevelCase & {
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
/** A native book that must be rejected, tagged with the QuotingError kind. */
type Rejection = {
  name: string;
  error: string;
  reference_bits: number;
  base_atoms: number;
  quote_atoms: number;
  asks: NativeLevelCase[];
  bids: NativeLevelCase[];
};

const vectors = JSON.parse(
  readFileSync(new URL('../../conformance/quoting_vectors.json', import.meta.url), 'utf8'),
) as { cases: Case[]; rejections: Rejection[] };

const LEVEL_BYTES = 10;

/**
 * Map each thrown {@link QuotingError} message to the canonical tag the
 * vectors carry — the same set of variants the Rust fork's `QuotingError`
 * enum names. A fork that rejected for a different reason maps to a
 * different tag and fails the assertion.
 */
const ERROR_TAGS: Record<string, string> = {
  'reference price is not a regular price': 'InvalidReference',
  'ask priced at or below reference': 'AskBelowReference',
  'bid priced at or above reference': 'BidAboveReference',
  'price offset overflows u32': 'OffsetOverflow',
  'inventory leg is zero': 'SizeExceedsInventory',
  'level size exceeds inventory leg': 'SizeExceedsInventory',
  'per-side Σ size_bps exceeds 10000': 'SizeExceedsInventory',
  [`more than ${N_LEVELS} levels on a side`]: 'TooManyLevels',
};

function nativeLevel(c: NativeLevelCase): NativeLevel {
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

test('quoting rejection vectors', () => {
  for (const r of vectors.rejections) {
    const book: NativeBook = {
      asks: r.asks.map(nativeLevel),
      bids: r.bids.map(nativeLevel),
    };
    let thrown: unknown;
    try {
      nativeBookToProfileBytes(book, r.reference_bits, BigInt(r.base_atoms), BigInt(r.quote_atoms));
    } catch (e) {
      thrown = e;
    }
    // The translation must reject — never clamp — so a value (no throw) fails.
    assert.ok(thrown instanceof QuotingError, `${r.name}: expected a QuotingError`);
    const message = (thrown as QuotingError).message;
    assert.equal(ERROR_TAGS[message], r.error, `${r.name}: error kind (message: "${message}")`);
  }
});
