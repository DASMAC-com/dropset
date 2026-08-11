/**
 * Router tests — the two routing rules (eligibility, tie-break) and the DFlow
 * client's wire handling.
 *
 * These deliberately avoid the WASM simulator: the matching math it runs is
 * already pinned by the conformance vectors, and what needs covering here is
 * the *routing* logic layered on top of it. Both rules are exported as pure
 * functions precisely so they can be exercised without a market blob.
 */

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { address } from '@solana/kit';
import { findAssociatedTokenPda } from '@solana-program/token';
import {
  DflowError,
  extractDflowApiError,
  fetchDflowQuote,
  parseDflowQuote,
  resolvePlatformFee,
} from './dflow';
import {
  type AggregatorQuote,
  type Candidate,
  classifyEclobQuote,
  type EclobQuote,
  NoRouteError,
  quoteBestRoute,
  selectBestRoute,
} from './router';

// A stand-in market address; nothing in these tests decodes it.
const MARKET = address('B1TFa9U1Rc4hVX1jkPmT4WoxAKN9nEZbrpKPjt6QRQGV');
const FEE_WALLET = address('9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM');
const MINT = address('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
const TOKEN_PROGRAM = address('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA');

const eclobQuote = (inAmount: bigint, outAmount: bigint): EclobQuote => ({
  venue: 'dropset',
  inAmount,
  outAmount,
  feeAmount: 0n,
  platformFeeAmount: 0n,
  platformFeeBps: 0,
  legs: 1,
  // The route is opaque to the routing rules under test.
  route: { market: MARKET } as unknown as EclobQuote['route'],
});

const aggQuote = (inAmount: bigint, outAmount: bigint): AggregatorQuote => ({
  venue: 'dflow',
  inAmount,
  outAmount,
  priceImpactPct: null,
  slippageBps: null,
  platformFee: null,
});

const quoted = <Q>(quote: Q): Candidate<Q> => ({
  status: 'quoted',
  quote,
  reason: null,
});
const missing = <Q>(reason: string): Candidate<Q> => ({
  status: 'unavailable',
  quote: null,
  reason,
});

// --- eligibility rule ------------------------------------------------------

test('a full-fill eCLOB quote is eligible', () => {
  const c = classifyEclobQuote(eclobQuote(1000n, 990n), 1000n);
  assert.equal(c.status, 'quoted');
  assert.equal(c.reason, null);
});

test('a partial fill is reported but does not compete', () => {
  const c = classifyEclobQuote(eclobQuote(400n, 402n), 1000n);
  assert.equal(c.status, 'partial');
  // The quote is still attached so a caller can surface what our book *would*
  // have done, even though it is not eligible to win.
  assert.equal(c.quote?.outAmount, 402n);
});

test('a zero-output quote is a liquidity failure, not a partial', () => {
  const c = classifyEclobQuote(eclobQuote(0n, 0n), 1000n);
  assert.equal(c.status, 'failed');
});

test('no market resolves to unavailable', () => {
  const c = classifyEclobQuote(null, 1000n);
  assert.equal(c.status, 'unavailable');
  assert.match(c.reason ?? '', /no Dropset market/);
});

// --- tie-break rule --------------------------------------------------------

test('the larger net output wins', () => {
  const best = selectBestRoute(
    quoted(eclobQuote(1000n, 980n)),
    quoted(aggQuote(1000n, 999n)),
  );
  assert.equal(best.venue, 'dflow');
  assert.equal(best.outAmount, 999n);
});

test('our own book wins when it is better', () => {
  const best = selectBestRoute(
    quoted(eclobQuote(1000n, 1001n)),
    quoted(aggQuote(1000n, 999n)),
  );
  assert.equal(best.venue, 'dropset');
});

test('a tie goes to our own book', () => {
  const best = selectBestRoute(
    quoted(eclobQuote(1000n, 999n)),
    quoted(aggQuote(1000n, 999n)),
  );
  assert.equal(best.venue, 'dropset');
});

test('a partial eCLOB quote loses to a worse aggregator quote', () => {
  // The partial's raw output (402) exceeds nothing here, but the point is that
  // even a *better-looking* partial must not win: it spends less input.
  const partial = classifyEclobQuote(eclobQuote(400n, 402n), 1000n);
  const best = selectBestRoute(partial, quoted(aggQuote(1000n, 399n)));
  assert.equal(best.venue, 'dflow');
});

test('a single available leg wins by default', () => {
  const best = selectBestRoute(
    missing<EclobQuote>('no Dropset market for this pair'),
    quoted(aggQuote(1000n, 999n)),
  );
  assert.equal(best.venue, 'dflow');
});

test('no eligible leg throws NoRouteError carrying both reasons', () => {
  assert.throws(
    () =>
      selectBestRoute(
        missing<EclobQuote>('no Dropset market for this pair'),
        missing<AggregatorQuote>('offline'),
      ),
    (e: unknown) => {
      assert.ok(e instanceof NoRouteError);
      assert.match(e.message, /Dropset: no Dropset market/);
      assert.match(e.message, /DFlow: offline/);
      return true;
    },
  );
});

// --- DFlow wire handling ---------------------------------------------------

test('parseDflowQuote rejects a non-integer amount without echoing it', () => {
  assert.throws(
    () => parseDflowQuote({ inAmount: '1e9', outAmount: '5' }),
    (e: unknown) => {
      assert.ok(e instanceof DflowError);
      assert.equal(e.kind, 'api');
      assert.match(e.message, /quote\.inAmount is not a valid integer/);
      assert.doesNotMatch(e.message, /1e9/);
      return true;
    },
  );
});

test('parseDflowQuote rejects a negative amount', () => {
  assert.throws(
    () => parseDflowQuote({ inAmount: '100', outAmount: '-5' }),
    /quote\.outAmount must be non-negative/,
  );
});

test('parseDflowQuote surfaces the echoed platform fee', () => {
  const q = parseDflowQuote({
    inAmount: '1000',
    outAmount: '990',
    priceImpactPct: '0.01',
    slippageBps: 50,
    platformFee: { amount: '5', feeBps: 50, feeAccount: 'abc' },
  });
  assert.equal(q.outAmount, 990n);
  assert.equal(q.slippageBps, 50);
  assert.equal(q.platformFee?.amount, 5n);
});

test('extractDflowApiError prefers the JSON message, else previews the body', async () => {
  const withMsg = await extractDflowApiError(
    new Response(JSON.stringify({ msg: 'bad mint', code: 'invalid_input_mint' }), {
      status: 400,
    }),
  );
  assert.equal(withMsg.message, 'bad mint');
  assert.equal(withMsg.code, 'invalid_input_mint');

  const html = await extractDflowApiError(
    new Response('<!DOCTYPE html><title>502</title>', { status: 502 }),
  );
  assert.match(html.message, /^HTTP 502: <!DOCTYPE html/);
  assert.equal(html.code, null);
});

/** Swap in a stub `fetch` for one call, restoring the real one afterwards. */
async function withFetch<T>(
  stub: (url: string, init?: RequestInit) => Promise<Response>,
  body: () => Promise<T>,
): Promise<T> {
  const real = globalThis.fetch;
  globalThis.fetch = ((url: string | URL, init?: RequestInit) =>
    stub(String(url), init)) as typeof fetch;
  try {
    return await body();
  } finally {
    globalThis.fetch = real;
  }
}

const okQuote = () =>
  new Response(JSON.stringify({ inAmount: '1000', outAmount: '990' }), {
    status: 200,
  });

test('a resolved platform fee is declared, pinned to the output mint', async () => {
  let seen = '';
  await withFetch(
    async (url) => {
      seen = url;
      return okQuote();
    },
    () =>
      fetchDflowQuote({
        quoteUrl: 'https://example.test/quote',
        inputMint: 'A',
        outputMint: 'B',
        amount: 1000n,
        slippageBps: 'auto',
        platformFee: { bps: 50, feeAccount: MINT },
      }),
  );
  const params = new URL(seen).searchParams;
  assert.equal(params.get('platformFeeBps'), '50');
  assert.equal(params.get('feeAccount'), MINT);
  // Pinned rather than left to the server default, since feeAccount is an ATA
  // of the output mint and the two must agree.
  assert.equal(params.get('platformFeeMode'), 'outputMint');
});

test('no fee params are sent when the fee is absent', async () => {
  let seen = '';
  await withFetch(
    async (url) => {
      seen = url;
      return okQuote();
    },
    () =>
      fetchDflowQuote({
        quoteUrl: 'https://example.test/quote',
        inputMint: 'A',
        outputMint: 'B',
        amount: 1000n,
        slippageBps: 'auto',
        platformFee: null,
      }),
  );
  const params = new URL(seen).searchParams;
  assert.equal(params.get('platformFeeBps'), null);
  assert.equal(params.get('feeAccount'), null);
  assert.equal(params.get('platformFeeMode'), null);
});

test('a 429 is surfaced as rateLimited so callers can back off', async () => {
  await withFetch(
    async () => new Response('slow down', { status: 429 }),
    async () => {
      await assert.rejects(
        fetchDflowQuote({
          quoteUrl: 'https://example.test/quote',
          inputMint: 'A',
          outputMint: 'B',
          amount: 1000n,
          slippageBps: 'auto',
        }),
        (e: unknown) => {
          assert.ok(e instanceof DflowError);
          assert.equal(e.kind, 'rateLimited');
          assert.equal(e.httpStatus, 429);
          return true;
        },
      );
    },
  );
});

test('a transport failure is a network error, not an api error', async () => {
  await withFetch(
    async () => {
      throw new TypeError('Failed to fetch');
    },
    async () => {
      await assert.rejects(
        fetchDflowQuote({
          quoteUrl: 'https://example.test/quote',
          inputMint: 'A',
          outputMint: 'B',
          amount: 1000n,
          slippageBps: 'auto',
        }),
        (e: unknown) => {
          assert.ok(e instanceof DflowError);
          assert.equal(e.kind, 'network');
          return true;
        },
      );
    },
  );
});

// --- the fee guard ---------------------------------------------------------

/** Minimal RPC stub: `getAccountInfo` reports which addresses exist. */
const rpcWith = (existing: Set<string>) =>
  ({
    getAccountInfo: (addr: string) => ({
      send: async () => ({
        value: existing.has(addr)
          ? {
              data: ['', 'base64'],
              executable: false,
              lamports: 1n,
              owner: TOKEN_PROGRAM,
              space: 0n,
            }
          : null,
      }),
    }),
  }) as never;

test('no fee is declared when the fee ATA does not exist', async () => {
  const resolved = await resolvePlatformFee(rpcWith(new Set()), {
    fee: { bps: 50, wallet: FEE_WALLET },
    mint: MINT,
    tokenProgram: TOKEN_PROGRAM,
  });
  // Skipping the fee keeps the route working; declaring one whose vault is
  // missing would break /order and waste slippage budget on /quote.
  assert.equal(resolved, null);
});

test('an existing fee ATA yields the fee, keyed to the fee wallet and mint', async () => {
  // The positive half of the guard, and the security-relevant one: the fee is
  // only chargeable when the vault is there, and `feeAccount` must be *that*
  // wallet's ATA for *that* mint — a wrong derivation would silently send the
  // fee somewhere else (or break /order).
  const [expected] = await findAssociatedTokenPda({
    owner: FEE_WALLET,
    mint: MINT,
    tokenProgram: TOKEN_PROGRAM,
  });
  const resolved = await resolvePlatformFee(rpcWith(new Set([expected])), {
    fee: { bps: 50, wallet: FEE_WALLET },
    mint: MINT,
    tokenProgram: TOKEN_PROGRAM,
  });
  assert.equal(resolved?.bps, 50);
  assert.equal(resolved?.feeAccount, expected);
});

test('the token program is read from the mint when not supplied', async () => {
  // The branch a caller takes when it doesn't already know the program: the
  // mint account's owner is used to derive the ATA.
  const [expected] = await findAssociatedTokenPda({
    owner: FEE_WALLET,
    mint: MINT,
    tokenProgram: TOKEN_PROGRAM,
  });
  const resolved = await resolvePlatformFee(
    rpcWith(new Set([MINT, expected])),
    { fee: { bps: 50, wallet: FEE_WALLET }, mint: MINT },
  );
  assert.equal(resolved?.feeAccount, expected);
});

test('a zero-bps fee is never declared', async () => {
  const resolved = await resolvePlatformFee(rpcWith(new Set()), {
    fee: { bps: 0, wallet: FEE_WALLET },
    mint: MINT,
  });
  assert.equal(resolved, null);
});

test('a null fee config resolves to no fee', async () => {
  const resolved = await resolvePlatformFee(rpcWith(new Set()), {
    fee: null,
    mint: MINT,
  });
  assert.equal(resolved, null);
});

// --- quoteBestRoute orchestration ------------------------------------------
//
// Driven aggregator-only (`eclob: null`) so these run without the WASM
// simulator: with no eCLOB leg and a pre-resolved fee, the RPC is never
// touched, and the aggregator half is the stubbed `fetch` above.

const noRpc = {} as never;

const bestRouteAggregatorOnly = (amount: bigint) =>
  quoteBestRoute(noRpc, {
    amount,
    eclob: null,
    aggregator: {
      quoteUrl: 'https://example.test/quote',
      inputMint: 'A',
      outputMint: 'B',
      slippageBps: 'auto',
      platformFee: null,
    },
  });

test('an aggregator-only route returns the aggregator as the winner', async () => {
  const { best, eclob } = await withFetch(
    async () => okQuote(),
    () => bestRouteAggregatorOnly(1000n),
  );
  assert.equal(best.venue, 'dflow');
  assert.equal(best.outAmount, 990n);
  // A leg that wasn't requested is reported as such, not as a failure.
  assert.equal(eclob.status, 'unavailable');
  assert.equal(eclob.reason, 'not requested');
});

test('a rate limit reaches the caller as a NoRouteError carrying the kind', async () => {
  // This is the contract the frontend's back-off reads: it inspects
  // `aggregator.cause` to decide whether to pause the poll chain.
  await withFetch(
    async () => new Response('slow down', { status: 429 }),
    async () => {
      await assert.rejects(bestRouteAggregatorOnly(1000n), (e: unknown) => {
        assert.ok(e instanceof NoRouteError);
        assert.equal(e.aggregator.status, 'failed');
        assert.ok(e.aggregator.cause instanceof DflowError);
        assert.equal(e.aggregator.cause.kind, 'rateLimited');
        return true;
      });
    },
  );
});

test('a zero-output aggregator quote is a failure, not a winner', async () => {
  await withFetch(
    async () =>
      new Response(JSON.stringify({ inAmount: '1000', outAmount: '0' }), {
        status: 200,
      }),
    async () => {
      await assert.rejects(bestRouteAggregatorOnly(1000n), (e: unknown) => {
        assert.ok(e instanceof NoRouteError);
        assert.match(e.aggregator.reason ?? '', /no output/);
        return true;
      });
    },
  );
});

test('an aggregator quote that underspends the input does not compete', async () => {
  // Symmetric with the eCLOB full-fill rule — an underspending quote is a
  // different trade, so it is reported but never wins.
  await withFetch(
    async () =>
      new Response(JSON.stringify({ inAmount: '400', outAmount: '402' }), {
        status: 200,
      }),
    async () => {
      await assert.rejects(bestRouteAggregatorOnly(1000n), (e: unknown) => {
        assert.ok(e instanceof NoRouteError);
        assert.equal(e.aggregator.status, 'partial');
        assert.equal(e.aggregator.quote?.outAmount, 402n);
        return true;
      });
    },
  );
});
