/**
 * Pin the two expiry clocks through the router's eCLOB leg.
 *
 * Level expiry is dual-domain — a level rests only inside **both** its slot
 * deadline and its wall-clock deadline — and `quoteBestRoute` takes the two
 * halves nested on one object (`GatedEclobLeg`), which it unwraps into
 * `quoteEclob` and on into the simulator. That unwrapping is plumbing, and
 * mis-plumbed plumbing here is a *silently wrong answer* rather than a crash: a
 * level judged against the wrong clock either rests when it should have expired
 * or vanishes while it was still live, and the quote that comes back looks
 * perfectly ordinary either way.
 *
 * `tsc` catches a straight transposition, because the two fields' optionality
 * differs — `nowSlot` is optional (a chain read fills it) and `nowUnix` is not,
 * so `nowUnix: gated.nowSlot` will not compile. What it cannot catch is **both
 * fields sourced from the same member**: `nowSlot: gated.nowUnix` widens a
 * `number` into `number | undefined` quite happily. Only a test catches that,
 * and only if the two clocks are *distinct* and each gate is observed on its
 * own — one combined assertion cannot distinguish the two directions, since
 * each mis-plumb is invisible in exactly the case that exposes the other.
 *
 * Unlike the rest of `router.test.ts`, these drive the real WASM simulator. The
 * gates live inside it, so there is nowhere else the plumbing can be observed
 * arriving.
 *
 * The book is the shared conformance fixture, whose levels carry both
 * deadlines: live at slot <= 999 and at unix <= 1_700_000_599, dead at either
 * boundary. Only the `market_data` blob is read from that file here; the two
 * deadlines and the expected fill below are transcribed from the
 * `expiry_slot_boundary_*` / `expiry_wall_boundary_*` vectors that pin those
 * edges — so they are sourced rather than guessed, and a regenerated fixture
 * fails this file loudly instead of quietly weakening it. The two domains are
 * three orders of magnitude apart, which is what makes a same-member mis-plumb
 * show up as a changed outcome instead of a coincidence.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { address } from '@solana/kit';

import { PRICE_INFINITY } from './price';
import type { EclobRoute } from './route';
import { NoRouteError, quoteBestRoute } from './router';
import { initSimulator } from './simulate';

const vectors = JSON.parse(
  readFileSync(
    new URL('../../conformance/simulate_swap_vectors.json', import.meta.url),
    'utf8',
  ),
) as { market_data: number[] };

await initSimulator(
  readFileSync(new URL('./wasm/dropset_interface_bg.wasm', import.meta.url)),
);

// The fixture book's deadlines, as pinned by the `expiry_*_boundary_*` vectors:
// the last slot and the last second at which its levels still rest.
const LAST_LIVE_SLOT = 999;
const LAST_LIVE_UNIX = 1_700_000_599;

// A take small enough to clear inside the top level, so the outcome is a clean
// full fill when the book is live and a flat zero when it is not.
const AMOUNT = 500_000n;
const FILLED_OUT = 458_089n;

const MARKET = address('B1TFa9U1Rc4hVX1jkPmT4WoxAKN9nEZbrpKPjt6QRQGV');
const BASE_MINT = address('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
const QUOTE_MINT = address('9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM');
const TOKEN_PROGRAM = address('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA');

// Pre-resolved, so the quote needs no market discovery: the only RPC the path
// can reach for is `getSlot`, which is exactly what we want to watch.
const ROUTE: EclobRoute = {
  market: MARKET,
  marketData: Uint8Array.from(vectors.market_data),
  baseMint: BASE_MINT,
  quoteMint: QUOTE_MINT,
  baseTokenProgram: TOKEN_PROGRAM,
  quoteTokenProgram: TOKEN_PROGRAM,
  side: 'buy',
  limitPriceBits: PRICE_INFINITY,
  outputMint: BASE_MINT,
  outputTokenProgram: TOKEN_PROGRAM,
  maxPlatformFeeBps: 100,
};

/**
 * The chain-read fallback, wired to fail loudly. A caller that supplies
 * `nowSlot` must never reach it, and a silent stub could not tell the
 * difference between "not consulted" and "not wired" — so it throws, and the
 * counter makes the absence of a read a positive assertion rather than a hope.
 */
const CHAIN_SLOT_READ = 'chain slot read';
let slotReads = 0;
const rpc = {
  getSlot: () => ({
    send: async (): Promise<never> => {
      slotReads += 1;
      throw new Error(CHAIN_SLOT_READ);
    },
  }),
} as never;

/** Quote our own book alone, with the two clocks set independently. */
const quoteGated = (nowSlot: number | undefined, nowUnix: number) =>
  quoteBestRoute(rpc, {
    amount: AMOUNT,
    eclob: { leg: { route: ROUTE }, nowSlot, nowUnix },
    aggregator: null,
  });

test('each clock reaches its own gate, and neither is a chain read', async () => {
  // Both clocks sit on their *last live* value, so the fill proves each gate
  // was fed its own domain: feed the wall clock to the slot gate and the book
  // is 1.7 billion slots expired, which zeroes the quote instead.
  const before = slotReads;
  const { best, eclob } = await quoteGated(LAST_LIVE_SLOT, LAST_LIVE_UNIX);
  assert.equal(eclob.status, 'quoted');
  assert.equal(best.venue, 'dropset');
  assert.equal(best.inAmount, AMOUNT);
  assert.equal(best.outAmount, FILLED_OUT);
  // The caller's slot was used verbatim, not refreshed behind its back — which
  // would quote a different book than the one the caller bounded.
  assert.equal(slotReads, before);
});

test('the wall clock alone can expire the book', async () => {
  // Slot still live, wall clock one second past the deadline. This is the half
  // that catches the wall gate being fed the *slot*: 999 is far inside the
  // wall deadline, so a same-member mis-plumb would fill here.
  await assert.rejects(
    quoteGated(LAST_LIVE_SLOT, LAST_LIVE_UNIX + 1),
    (e: unknown) => {
      assert.ok(e instanceof NoRouteError);
      assert.equal(e.eclob.status, 'failed');
      // Pinning the reason, not just the status, is what separates an emptied
      // book from the other ways this leg can fail: a chain-read fallback or a
      // simulator throw both land in `failed` too, with a different reason.
      assert.match(e.eclob.reason ?? '', /no liquidity/);
      return true;
    },
  );
});

test('the slot clock alone can expire the book', async () => {
  // The exact mirror of the test above: wall clock on its last live value,
  // slot one past. This is the half that catches the slot gate being fed the
  // *wall* clock.
  await assert.rejects(
    quoteGated(LAST_LIVE_SLOT + 1, LAST_LIVE_UNIX),
    (e: unknown) => {
      assert.ok(e instanceof NoRouteError);
      assert.equal(e.eclob.status, 'failed');
      assert.match(e.eclob.reason ?? '', /no liquidity/);
      return true;
    },
  );
});

test('an omitted slot falls back to the chain read', async () => {
  // The fallback is real and reachable, which is what makes its silence in the
  // three tests above evidence of anything. A failure here means the stub
  // stopped being wired, and those assertions quietly stopped proving it.
  const before = slotReads;
  await assert.rejects(quoteGated(undefined, LAST_LIVE_UNIX), (e: unknown) => {
    assert.ok(e instanceof NoRouteError);
    assert.equal(e.eclob.status, 'failed');
    assert.match(e.eclob.reason ?? '', new RegExp(CHAIN_SLOT_READ));
    return true;
  });
  assert.equal(slotReads, before + 1);
});
