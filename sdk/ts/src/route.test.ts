/**
 * eCLOB route-resolution tests — the two-orientation search and the fee clamp.
 *
 * A pair has two possible market orientations and the take side follows from
 * whichever one exists on-chain, so every field of a resolved route is derived
 * from that choice: `side`, the unbounded `limitPriceBits`, and — the
 * security-relevant one — `outputMint` / `outputTokenProgram`, which the
 * platform fee is paid in. Getting the output leg wrong yields an ATA the
 * program's `create_idempotent` CPI rejects, so the orientation logic is
 * asserted from both directions rather than only the common one.
 *
 * The RPC is a structural stub: `resolveEclobRoute` only needs `getAccountInfo`,
 * and serving the conformance vectors' real market bytes means the header decode
 * that yields `maxPlatformFeeBps` is exercised for real rather than mocked.
 *
 * Run: `pnpm --filter @dropset/sdk test`.
 */

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { type Address, address } from '@solana/kit';

import { findMarketPda } from './generated';
import { PRICE_INFINITY, PRICE_ZERO } from './price';
import { platformFeeBpsFor, resolveEclobRoute } from './route';

const FROM = address('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
const TO = address('HzwqbKZw8HxMN6bF2yFZNrht3c2iXXzpKcFu7uBEDKtr');
const TOKEN_PROGRAM = address('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA');
const TOKEN_2022_PROGRAM = address(
  'TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb',
);

const vectors = JSON.parse(
  readFileSync(
    new URL('../../conformance/simulate_swap_vectors.json', import.meta.url),
    'utf8',
  ),
) as { market_data: number[] };

const MARKET_BYTES = Uint8Array.from(vectors.market_data);
const MARKET_BASE64 = Buffer.from(MARKET_BYTES).toString('base64');

// The ceiling encoded in the vectors' market header. Pinned as a literal so the
// header decode is actually asserted — the vectors' own
// `buy_platform_fee_over_ceiling` case (101 bps → all-zero quote) is what fixes
// it at 100.
const MARKET_MAX_PLATFORM_FEE_BPS = 100;

// Stand-in market addresses for the fee-clamp tests, which never resolve a
// route and so need no real PDA — only two addresses that differ from each
// other, since the clamp warning dedups per market across the whole file.
const MARKET_A = address('So11111111111111111111111111111111111111112');
const MARKET_B = address('ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL');

type StubAccount = { data: string; owner: Address };

/**
 * Minimal `getAccountInfo` stub. Addresses absent from `accounts` report as
 * non-existent, which is how the orientation search is steered. When `calls` is
 * passed, every queried address is appended to it so a test can assert the
 * *number* of reads, not just the result.
 */
const rpcWith = (accounts: Map<string, StubAccount>, calls?: string[]) =>
  ({
    getAccountInfo: (addr: string) => ({
      send: async () => {
        calls?.push(addr);
        const account = accounts.get(addr);
        return {
          value: account
            ? {
                data: [account.data, 'base64'],
                executable: false,
                lamports: 1n,
                owner: account.owner,
                space: BigInt(MARKET_BYTES.length),
              }
            : null,
        };
      },
    }),
  }) as never;

const marketAccount = (owner: Address = TOKEN_PROGRAM): StubAccount => ({
  data: MARKET_BASE64,
  owner,
});

/** The market PDA for one orientation. */
const marketFor = async (
  baseMint: Address,
  quoteMint: Address,
): Promise<Address> => {
  const [market] = await findMarketPda({ baseMint, quoteMint });
  return market;
};

test('resolves the sell orientation when the from-token is the base', async () => {
  const market = await marketFor(FROM, TO);
  const route = await resolveEclobRoute(
    rpcWith(new Map([[market, marketAccount()]])),
    {
      inputMint: FROM,
      outputMint: TO,
      inputTokenProgram: TOKEN_PROGRAM,
      outputTokenProgram: TOKEN_2022_PROGRAM,
    },
  );

  assert.ok(route, 'expected a resolved route');
  assert.equal(route.market, market);
  assert.equal(route.side, 'sell');
  // Spending the base means taking down bids: no lower bound.
  assert.equal(route.limitPriceBits, PRICE_ZERO);
  assert.equal(route.baseMint, FROM);
  assert.equal(route.quoteMint, TO);
  assert.equal(route.baseTokenProgram, TOKEN_PROGRAM);
  assert.equal(route.quoteTokenProgram, TOKEN_2022_PROGRAM);
  // On a sell the taker receives the *quote* leg.
  assert.equal(route.outputMint, TO);
  assert.equal(route.outputTokenProgram, TOKEN_2022_PROGRAM);
  assert.equal(route.maxPlatformFeeBps, MARKET_MAX_PLATFORM_FEE_BPS);
  // Handed over verbatim, discriminator included, so the quoter needs no
  // second fetch.
  assert.deepEqual(route.marketData, MARKET_BYTES);
});

test('resolves the buy orientation when only the reverse market exists', async () => {
  const market = await marketFor(TO, FROM);
  const route = await resolveEclobRoute(
    rpcWith(new Map([[market, marketAccount()]])),
    {
      inputMint: FROM,
      outputMint: TO,
      inputTokenProgram: TOKEN_PROGRAM,
      outputTokenProgram: TOKEN_2022_PROGRAM,
    },
  );

  assert.ok(route, 'expected a resolved route');
  assert.equal(route.market, market);
  assert.equal(route.side, 'buy');
  // Spending the quote means lifting asks: no upper bound.
  assert.equal(route.limitPriceBits, PRICE_INFINITY);
  // The orientation is flipped, and so are the programs that travel with it.
  assert.equal(route.baseMint, TO);
  assert.equal(route.quoteMint, FROM);
  assert.equal(route.baseTokenProgram, TOKEN_2022_PROGRAM);
  assert.equal(route.quoteTokenProgram, TOKEN_PROGRAM);
  // On a buy the taker receives the *base* leg — which is still the caller's
  // to-token, as in the sell case. Both orientations must agree on that.
  assert.equal(route.outputMint, TO);
  assert.equal(route.outputTokenProgram, TOKEN_2022_PROGRAM);
});

test('the output leg is the caller to-token in either orientation', async () => {
  const sellMarket = await marketFor(FROM, TO);
  const buyMarket = await marketFor(TO, FROM);
  const input = {
    inputMint: FROM,
    outputMint: TO,
    inputTokenProgram: TOKEN_PROGRAM,
    outputTokenProgram: TOKEN_2022_PROGRAM,
  };

  const sell = await resolveEclobRoute(
    rpcWith(new Map([[sellMarket, marketAccount()]])),
    input,
  );
  const buy = await resolveEclobRoute(
    rpcWith(new Map([[buyMarket, marketAccount()]])),
    input,
  );

  assert.equal(sell?.side, 'sell');
  assert.equal(buy?.side, 'buy');
  // The invariant the platform-fee ATA derivation depends on.
  assert.equal(sell?.outputMint, TO);
  assert.equal(buy?.outputMint, TO);
  assert.equal(sell?.outputTokenProgram, TOKEN_2022_PROGRAM);
  assert.equal(buy?.outputTokenProgram, TOKEN_2022_PROGRAM);
});

test('short-circuits on the first orientation that exists', async () => {
  // Both markets exist; the search must stop at the first and not pay for the
  // second. This is on the quote timer, so the extra read is per-tick waste.
  const sellMarket = await marketFor(FROM, TO);
  const buyMarket = await marketFor(TO, FROM);
  const calls: string[] = [];
  const route = await resolveEclobRoute(
    rpcWith(
      new Map([
        [sellMarket, marketAccount()],
        [buyMarket, marketAccount()],
      ]),
      calls,
    ),
    {
      inputMint: FROM,
      outputMint: TO,
      inputTokenProgram: TOKEN_PROGRAM,
      outputTokenProgram: TOKEN_PROGRAM,
    },
  );

  assert.equal(route?.side, 'sell');
  assert.deepEqual(calls, [sellMarket]);
});

test('returns null when neither orientation has a market', async () => {
  const calls: string[] = [];
  const route = await resolveEclobRoute(rpcWith(new Map(), calls), {
    inputMint: FROM,
    outputMint: TO,
    inputTokenProgram: TOKEN_PROGRAM,
    outputTokenProgram: TOKEN_PROGRAM,
  });

  // An ordinary outcome, not an error: the pair simply isn't deployed here.
  assert.equal(route, null);
  // Both orientations were tried before giving up.
  assert.equal(calls.length, 2);
});

test('returns null for a degenerate pair without touching the RPC', async () => {
  const calls: string[] = [];
  const route = await resolveEclobRoute(rpcWith(new Map(), calls), {
    inputMint: FROM,
    outputMint: FROM,
  });

  assert.equal(route, null);
  assert.deepEqual(calls, []);
});

test('reads each token program from its mint when not supplied', async () => {
  // The branch a caller takes when it doesn't already know the programs. The two
  // mints are given *different* owners so a swapped assignment fails here.
  const market = await marketFor(FROM, TO);
  const route = await resolveEclobRoute(
    rpcWith(
      new Map([
        [market, marketAccount()],
        [FROM, { data: '', owner: TOKEN_PROGRAM }],
        [TO, { data: '', owner: TOKEN_2022_PROGRAM }],
      ]),
    ),
    { inputMint: FROM, outputMint: TO },
  );

  assert.ok(route, 'expected a resolved route');
  assert.equal(route.baseTokenProgram, TOKEN_PROGRAM);
  assert.equal(route.quoteTokenProgram, TOKEN_2022_PROGRAM);
});

test('a missing mint account is an error, not a silent null route', async () => {
  // Distinct from "no market for this pair": a mint that doesn't exist means the
  // caller is quoting against the wrong cluster, which should surface loudly
  // rather than look like a pair we never deployed.
  await assert.rejects(
    () =>
      resolveEclobRoute(rpcWith(new Map()), {
        inputMint: FROM,
        outputMint: TO,
      }),
    /does not exist on this cluster/,
  );
});

/**
 * A route stub carrying only what the fee clamp reads. The market address is a
 * dedup key for the clamp warning and nothing else, so any distinct address
 * serves — but it must differ between tests, since the "warn once" set is
 * module-level and outlives a single test.
 */
const feeRoute = (market: Address, maxPlatformFeeBps: number) =>
  ({ market, maxPlatformFeeBps }) as Parameters<typeof platformFeeBpsFor>[0];

/** Run `fn` with `console.warn` captured, and return what it emitted. */
function captureWarnings(fn: () => void): string[] {
  const emitted: string[] = [];
  const original = console.warn;
  console.warn = (...args: unknown[]) => {
    emitted.push(args.join(' '));
  };
  try {
    fn();
  } finally {
    console.warn = original;
  }
  return emitted;
}

test('the platform fee is clamped to the market ceiling', () => {
  const route = feeRoute(MARKET_A, 100);
  captureWarnings(() => {
    // Under the ceiling the configured rate is honoured.
    assert.equal(platformFeeBpsFor(route, 50), 50);
    // At it, unchanged.
    assert.equal(platformFeeBpsFor(route, 100), 100);
    // Above it, clamped rather than rejected — under-charging is the safe
    // direction, and refusing would surface as a broken quote.
    assert.equal(platformFeeBpsFor(route, 250), 100);
  });
});

test('a clamp warns once per market, naming both rates', () => {
  const route = feeRoute(MARKET_B, 10);

  // Nothing to report while the configured rate fits under the ceiling: the
  // clamp is a no-op there, and warning would cry wolf on a correct config.
  assert.deepEqual(
    captureWarnings(() => {
      assert.equal(platformFeeBpsFor(route, 10), 10);
    }),
    [],
  );

  // The first clamp reports it, naming the ceiling, the configured rate, and
  // the market — the three facts needed to tell which knob is wrong.
  const first = captureWarnings(() => {
    assert.equal(platformFeeBpsFor(route, 50), 10);
  });
  assert.equal(first.length, 1);
  assert.match(first[0] ?? '', /50 bps is\s+configured/);
  assert.match(first[0] ?? '', /max_platform_fee is 10 bps/);
  assert.ok(first[0]?.includes(MARKET_B));

  // Repeats stay silent. The quote loop re-clamps every few seconds for as
  // long as the pair is on screen, so warning each time would bury the console
  // — and a config fact only needs saying once.
  assert.deepEqual(
    captureWarnings(() => {
      assert.equal(platformFeeBpsFor(route, 50), 10);
      assert.equal(platformFeeBpsFor(route, 50), 10);
    }),
    [],
  );

  // A *different* configured rate is a different fact, so it is reported even
  // though this market has already warned once — an operator editing the rate
  // mid-session should hear about the new value.
  const afterEdit = captureWarnings(() => {
    assert.equal(platformFeeBpsFor(route, 75), 10);
  });
  assert.equal(afterEdit.length, 1);
  assert.match(afterEdit[0] ?? '', /75 bps is\s+configured/);
});
