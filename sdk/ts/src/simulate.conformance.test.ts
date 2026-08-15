/**
 * Pin the TypeScript `simulateSwap` wrapper to the shared conformance vectors.
 *
 * `wasm.conformance.test.ts` already drives the raw binding through
 * `sdk/conformance/simulate_swap_vectors.json`, which pins the committed
 * binary's *matching math*. What that leaves uncovered is the thin marshalling
 * layer in `./simulate` — and every part of it is a silent-wrong-answer risk
 * rather than a crash:
 *
 * - `SIDE_CODE` maps `'buy' | 'sell'` onto the binding's 0/1 discriminant.
 *   Transposed, every quote would price the opposite side of the book and still
 *   return plausible numbers.
 * - The result is rebuilt field-by-field from snake_case getters into camelCase.
 *   `feeAmount` and `platformFeeAmount` are adjacent, same-typed, and owed to
 *   *different parties*, so swapping them misreports whose money it is.
 * - `q.free()` runs in a `finally` after the fields are read out. Reading a
 *   freed handle throws, so the ordering is load-bearing.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { PRICE_INFINITY, PRICE_ZERO } from './price';
import { initSimulator, simulateSwap, type SwapSide } from './simulate';
import { simulate_swap as rawSimulateSwap } from './wasm/dropset_interface.js';

type ExpectedQuote = {
  in_amount: number;
  out_amount: number;
  fee_amount: number;
  platform_fee_amount: number;
  legs: number;
};
type SwapCase = {
  name: string;
  side: number;
  amount_in: number;
  limit_price_bits: number;
  now_slot: number;
  now_unix: number;
  platform_fee_bps: number;
  expected: ExpectedQuote;
};

const vectors = JSON.parse(
  readFileSync(
    new URL('../../conformance/simulate_swap_vectors.json', import.meta.url),
    'utf8',
  ),
) as { market_data: number[]; cases: SwapCase[] };

const marketData = Uint8Array.from(vectors.market_data);

/** The binding's discriminant → the wrapper's side name. */
const SIDE_BY_CODE: Record<number, SwapSide> = { 0: 'buy', 1: 'sell' };

// `initSimulator` resolves the `.wasm` asset relative to the module by default,
// which needs a bundler's asset pipeline; under Node we hand it the committed
// bytes. This also shares one instantiation with the raw binding imported
// above — both are views onto the same module instance — which is what lets the
// side-mapping test below compare the two directly.
await initSimulator(
  readFileSync(new URL('./wasm/dropset_interface_bg.wasm', import.meta.url)),
);

test('the wrapper marshals every conformance vector out unchanged', () => {
  for (const c of vectors.cases) {
    const side = SIDE_BY_CODE[c.side];
    assert.ok(side !== undefined, `${c.name}: unmapped side ${c.side}`);
    const q = simulateSwap(
      marketData,
      side,
      BigInt(c.amount_in),
      c.limit_price_bits,
      c.now_slot,
      c.now_unix,
      c.platform_fee_bps,
    );
    assert.equal(q.inAmount, BigInt(c.expected.in_amount), `${c.name}: inAmount`);
    assert.equal(
      q.outAmount,
      BigInt(c.expected.out_amount),
      `${c.name}: outAmount`,
    );
    assert.equal(
      q.feeAmount,
      BigInt(c.expected.fee_amount),
      `${c.name}: feeAmount`,
    );
    assert.equal(
      q.platformFeeAmount,
      BigInt(c.expected.platform_fee_amount),
      `${c.name}: platformFeeAmount`,
    );
    assert.equal(q.legs, c.expected.legs, `${c.name}: legs`);
  }
});

// The two fee fields are the transposition this guards: both bigint, adjacent in
// the struct, and non-zero *together* in only some vectors — so a swap would
// pass any test that only checked their sum. `buy_multi_level_platform_fee` has
// them an order of magnitude apart, which makes the assertion sharp.
test('the taker fee and the platform fee are not transposed', () => {
  const c = vectors.cases.find(
    (v) => v.name === 'buy_multi_level_platform_fee',
  );
  assert.ok(c, 'expected a buy_multi_level_platform_fee vector');
  const q = simulateSwap(
    marketData,
    'buy',
    BigInt(c.amount_in),
    c.limit_price_bits,
    c.now_slot,
    c.now_unix,
    c.platform_fee_bps,
  );
  assert.equal(q.feeAmount, 1_800n);
  assert.equal(q.platformFeeAmount, 17_982n);
  assert.notEqual(q.feeAmount, q.platformFeeAmount);
});

// Pin `SIDE_CODE` against the binding itself rather than against the vectors:
// this fails if the map is transposed even in the (hypothetical) case where both
// sides of the book happen to price alike.
test("side names map onto the binding's discriminants", () => {
  for (const [code, side] of Object.entries(SIDE_BY_CODE)) {
    const limitPriceBits = side === 'buy' ? PRICE_INFINITY : PRICE_ZERO;
    const raw = rawSimulateSwap(
      marketData,
      Number(code),
      1_000_000n,
      limitPriceBits,
      1,
      1,
      0,
    );
    const viaWrapper = simulateSwap(
      marketData,
      side,
      1_000_000n,
      limitPriceBits,
      1,
      1,
      0,
    );
    assert.equal(viaWrapper.inAmount, raw.in_amount, `${side}: inAmount`);
    assert.equal(viaWrapper.outAmount, raw.out_amount, `${side}: outAmount`);
    assert.equal(viaWrapper.feeAmount, raw.fee_amount, `${side}: feeAmount`);
    assert.equal(viaWrapper.legs, raw.legs, `${side}: legs`);
    raw.free();
  }
});

// A buy and a sell against this book must not agree, or the test above would
// hold for a transposed map too.
test('the two sides price differently against the same book', () => {
  const buy = simulateSwap(marketData, 'buy', 1_000_000n, PRICE_INFINITY, 1, 0);
  const sell = simulateSwap(marketData, 'sell', 1_000_000n, PRICE_ZERO, 1, 0);
  assert.notEqual(buy.outAmount, sell.outAmount);
});

// The `finally { q.free() }` ordering: the returned value must be a fully
// materialized plain object, not a live view onto linear memory. If the free
// ran before the fields were read, this would throw rather than fail — and if
// the wrapper returned the wasm handle itself, the prototype check catches it.
//
// Leak-freedom itself is not observable from here (a missed `free()` leaks a JS
// heap slot without throwing), so this covers the ordering, which is the half
// that can produce a wrong answer.
test('the result is a materialized plain object, not a wasm handle', () => {
  const q = simulateSwap(marketData, 'buy', 500_000n, PRICE_INFINITY, 1, 0);
  assert.equal(Object.getPrototypeOf(q), Object.prototype);
  assert.equal(typeof q.inAmount, 'bigint');
  assert.equal(typeof q.outAmount, 'bigint');
  assert.equal(typeof q.feeAmount, 'bigint');
  assert.equal(typeof q.platformFeeAmount, 'bigint');
  assert.equal(typeof q.legs, 'number');
  // Readable again after the call returned — i.e. copied out, not proxied.
  // The take is exact-in and this one is bounded by the taker's own budget,
  // so `inAmount` is the whole 500_000 rather than the 499_999 the engine
  // could price into whole base atoms.
  assert.equal(q.inAmount, 500_000n);
  assert.deepEqual(
    { ...q },
    {
      inAmount: 500_000n,
      outAmount: 458_089n,
      feeAmount: 458n,
      platformFeeAmount: 0n,
      legs: 1,
    },
  );
});

// The sentinels the wrapper's docs tell callers to use for an unbounded take
// must be the values the vectors were generated with, or "market order" means
// something different to the quoter than it does to the engine.
test('the unbounded-take sentinels match the vectors', () => {
  const marketBuy = vectors.cases.find((v) => v.name === 'buy_multi_level');
  const marketSell = vectors.cases.find((v) => v.name === 'sell_multi_level');
  assert.ok(marketBuy && marketSell, 'expected the market-take vectors');
  assert.equal(marketBuy.limit_price_bits, PRICE_INFINITY);
  assert.equal(marketSell.limit_price_bits, PRICE_ZERO);
});
