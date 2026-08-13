/**
 * Pin the **committed** WASM binary to the shared conformance vectors.
 *
 * `sdk/ts/src/wasm/dropset_interface_bg.wasm` is a build artifact checked
 * into the source tree, because the frontend deploys with no Rust
 * toolchain and so cannot build it (publishing it as a package instead is
 * blocked on the SDK's unreleased anchor dependency). A committed artifact
 * goes stale, and neither existing CI guard catches that:
 *
 * - `.github/workflows/sdk.yml`'s "Committed WASM glue matches a fresh
 *   build" diffs only the generated `.js`/`.d.ts`. It deliberately skips
 *   the `.wasm` itself, since `wasm-opt` output is not byte-reproducible
 *   across the committer's platform and CI's. The glue is generated from
 *   exported *signatures*, so an account-layout change leaves it
 *   byte-identical while the binary's decode moves — and the diff passes.
 * - `wasm-pack test --node` (sdk/interface/tests/wasm_conformance.rs)
 *   compiles the binding fresh from source, so it proves the *current
 *   source* is conformant, never the committed binary.
 *
 * The gap shipped a stale binary whose `MarketHeader`/`Vault` layout
 * disagreed with the deployed program; the frontend's `simulate_swap` then
 * failed with `SectorOverflow` on a live market while CI stayed green.
 *
 * This test closes it by asserting on *behavior* rather than bytes — which
 * is both the property that matters and the only one available, given the
 * reproducibility constraint above. It loads the committed `.wasm` through
 * the committed glue and runs the same
 * `sdk/conformance/simulate_swap_vectors.json` the Rust-side binding test
 * uses, so a layout change landed without `make wasm` fails here.
 *
 * Ordering matters: sdk.yml runs `pnpm --filter @dropset/sdk test` *before*
 * its `make wasm` step, so this sees the committed artifact, not a
 * regenerated one.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { initSync, simulate_swap } from './wasm/dropset_interface.js';

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
  readFileSync(new URL('../../conformance/simulate_swap_vectors.json', import.meta.url), 'utf8'),
) as { market_data: number[]; cases: SwapCase[] };

// The glue is built `--target web`, whose default init fetches the binary
// relative to `import.meta.url`. Under Node we hand it the committed bytes
// directly — which is also precisely what makes this a test *of* that file.
initSync({
  module: readFileSync(new URL('./wasm/dropset_interface_bg.wasm', import.meta.url)),
});

const marketData = Uint8Array.from(vectors.market_data);

test('committed wasm simulate_swap matches the conformance vectors', () => {
  for (const c of vectors.cases) {
    const q = simulate_swap(
      marketData,
      c.side,
      BigInt(c.amount_in),
      c.limit_price_bits,
      c.now_slot,
      c.now_unix,
      c.platform_fee_bps,
    );
    assert.equal(q.in_amount, BigInt(c.expected.in_amount), `${c.name}: in_amount`);
    assert.equal(q.out_amount, BigInt(c.expected.out_amount), `${c.name}: out_amount`);
    assert.equal(q.fee_amount, BigInt(c.expected.fee_amount), `${c.name}: fee_amount`);
    assert.equal(
      q.platform_fee_amount,
      BigInt(c.expected.platform_fee_amount),
      `${c.name}: platform_fee_amount`,
    );
    assert.equal(q.legs, c.expected.legs, `${c.name}: legs`);
    q.free();
  }
});

// The decode boundary. A stale binary most often fails by *misreading* a
// current market rather than by refusing one, which the vectors above
// catch; this pins the other end — a buffer too short to hold the header
// must surface as a thrown error, not a panic across the wasm boundary.
// The matcher matters: a bare `assert.throws` passes on ANY throw, including
// the argument-marshalling errors a stale binary produces — so it would stay
// green while proving nothing.
test('committed wasm simulate_swap rejects an undersized buffer', () => {
  assert.throws(
    () => simulate_swap(new Uint8Array(4), 0, 1_000_000n, 0xffffffff, 1, 1, 0),
    /TooSmall/,
  );
});

// `side` has no native analogue (the matcher takes an enum), so the binding
// maps 0/1 itself and must reject anything else rather than silently
// picking a side.
test('committed wasm simulate_swap rejects an invalid side', () => {
  assert.throws(
    () => simulate_swap(marketData, 2, 1_000_000n, 0xffffffff, 1, 1, 0),
    /side/i,
  );
});
