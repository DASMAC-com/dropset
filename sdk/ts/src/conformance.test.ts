/**
 * Verify the TS `Price` codec against the shared conformance vectors —
 * the same `sdk/conformance/price_vectors.json` the Rust side checks
 * (sdk/price-core/tests/conformance.rs). This pins the TS hand-written
 * codec to the engine's arithmetic.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { baseForQuote, decodePrice, encodePrice, isValidPrice, quoteForBase } from './price';

type DecodeCase = { bits: number; valid: boolean; value: number | null };
type EncodeCase = { value: number; bits: number | null };
type RatioCase = { bits: number; base?: number; quote?: number; expected: number };

const U64_MAX = (1n << 64n) - 1n;

const vectors = JSON.parse(
  readFileSync(new URL('../../conformance/price_vectors.json', import.meta.url), 'utf8'),
) as {
  decode: DecodeCase[];
  encode: EncodeCase[];
  quote_for_base: RatioCase[];
  base_for_quote: RatioCase[];
};

test('decode vectors match', () => {
  for (const c of vectors.decode) {
    assert.equal(isValidPrice(c.bits), c.valid, `valid(${c.bits})`);
    const got = decodePrice(c.bits);
    if (c.value === null) {
      assert.ok(!Number.isFinite(got), `expected INFINITY sentinel for ${c.bits}`);
    } else {
      const tol = 1e-9 * Math.max(1, Math.abs(c.value));
      assert.ok(Math.abs(got - c.value) <= tol, `decode ${c.value} got ${got}`);
    }
  }
});

test('encode vectors match', () => {
  for (const c of vectors.encode) {
    assert.equal(encodePrice(c.value), c.bits, `encode ${c.value}`);
  }
});

// The ratio math (saturated to u64, as the vectors store it) backs the
// native-quoting offset; pinning it keeps the TS hand-mirror in lockstep
// with the Rust engine math.
const sat = (v: bigint) => (v > U64_MAX ? U64_MAX : v);

test('quote_for_base vectors match', () => {
  for (const c of vectors.quote_for_base) {
    const got = sat(quoteForBase(c.bits, BigInt(c.base ?? 0)));
    assert.equal(got, BigInt(c.expected), `quote_for_base(${c.bits}, ${c.base})`);
  }
});

test('base_for_quote vectors match', () => {
  for (const c of vectors.base_for_quote) {
    const got = sat(baseForQuote(c.bits, BigInt(c.quote ?? 0)));
    assert.equal(got, BigInt(c.expected), `base_for_quote(${c.bits}, ${c.quote})`);
  }
});
