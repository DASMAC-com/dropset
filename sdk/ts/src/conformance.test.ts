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

import { decodePrice, encodePrice, isValidPrice } from './price';

type DecodeCase = { bits: number; valid: boolean; value: number | null };
type EncodeCase = { value: number; bits: number | null };

const vectors = JSON.parse(
  readFileSync(new URL('../../conformance/price_vectors.json', import.meta.url), 'utf8'),
) as { decode: DecodeCase[]; encode: EncodeCase[] };

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
