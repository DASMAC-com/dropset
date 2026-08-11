/**
 * eCLOB route resolution — mapping a from→to mint pair onto a Dropset market.
 *
 * A Dropset market is a PDA of `[baseMint, quoteMint]`, and either token of a
 * pair could be the base, so a pair has two possible orientations and the take
 * side follows from whichever one exists on-chain:
 *
 *   - market(base=from, quote=to): the taker spends the base → a `sell`.
 *   - market(base=to,   quote=from): the taker spends the quote → a `buy`.
 *
 * The resolved {@link EclobRoute} carries everything both the quoter
 * ({@link ./simulate | simulateSwap}) and the swap-instruction builder need,
 * including the market bytes already fetched during discovery — so a caller
 * that resolves a route does not pay a second `getAccountInfo` to quote it.
 */

import {
  type Address,
  type FetchAccountConfig,
  fetchEncodedAccount,
} from '@solana/kit';
import {
  findMarketPda,
  getMarketHeaderDecoder,
  getMarketHeaderSize,
} from './generated';
import { PRICE_INFINITY, PRICE_ZERO, type PriceBits } from './price';
import type { SwapSide } from './simulate';

/** Minimal account-fetch RPC shape — structural, so any kit RPC satisfies it. */
export type AccountRpc = Parameters<typeof fetchEncodedAccount>[0];

/**
 * Minimal `getSlot` shape — the current slot scopes flush-level expiry. Kept
 * argument-less for structural compatibility with any kit RPC; a caller that
 * needs a commitment other than the client's default should read the slot
 * itself and pass it in.
 */
export type SlotRpc = {
  getSlot: (...args: never[]) => { send: () => Promise<bigint> };
};

/**
 * A resolved route against a market that actually exists on the current
 * cluster: the market PDA and its raw bytes, the take side, the no-bound limit
 * price, and the base/quote mints + token programs the swap instruction needs.
 */
export type EclobRoute = {
  market: Address;
  /** Raw account bytes, discriminator included — pass verbatim to `simulateSwap`. */
  marketData: Uint8Array;
  baseMint: Address;
  quoteMint: Address;
  baseTokenProgram: Address;
  quoteTokenProgram: Address;
  side: SwapSide;
  /** `PRICE_INFINITY` for a buy, `PRICE_ZERO` for a sell — an unbounded take. */
  limitPriceBits: PriceBits;
  /**
   * The leg the taker *receives* — base on a buy, quote on a sell. Derived here,
   * beside the `side` it follows from, rather than re-deduced at each use: the
   * platform fee is paid in this mint, and picking the wrong one yields an ATA
   * the program's `create_idempotent` CPI rejects. These are the on-chain mints
   * (mock demo mints on localnet), which is what the fee destination must be
   * derived against.
   */
  outputMint: Address;
  outputTokenProgram: Address;
  /**
   * This market's on-chain ceiling on a declared `platform_fee_bps`. The program
   * rejects any swap above it, so callers clamp to this rather than sending the
   * configured rate blind — see {@link platformFeeBpsFor}.
   */
  maxPlatformFeeBps: number;
};

/**
 * The platform fee a route may actually declare: the configured rate, clamped
 * to the market's own ceiling.
 *
 * Clamping rather than failing, because the two outcomes are not symmetric. If
 * an operator configures a rate above some market's ceiling, charging that
 * market's maximum earns slightly less than intended; refusing the swap breaks
 * trading on that pair outright and surfaces to the user as a broken quote (the
 * simulator returns an all-zero quote for an over-ceiling rate, which a UI would
 * report as "no liquidity" — a misleading diagnosis of what is really a
 * config/ceiling mismatch). Under-charging is the safe direction for a knob that
 * only sets our own revenue.
 */
export function platformFeeBpsFor(
  route: EclobRoute,
  configuredBps: number,
): number {
  return Math.min(configuredBps, route.maxPlatformFeeBps);
}

/**
 * The mints to route between, in **on-chain** terms: whatever mints actually
 * exist on the cluster being quoted (a localnet deployment's mock mints, the
 * real mints on mainnet). Token programs are read from the mint accounts when
 * not supplied — pass them to skip those reads when the caller already knows.
 */
export type EclobRouteInput = {
  inputMint: Address;
  outputMint: Address;
  inputTokenProgram?: Address;
  outputTokenProgram?: Address;
};

/**
 * One candidate orientation: the pair mapped onto a market plus the implied
 * side. Named for the orientation rather than "candidate" so it doesn't read as
 * the router's `Candidate<Q>`, which is a different thing entirely (a priced
 * leg and its verdict).
 */
type Orientation = {
  baseMint: Address;
  quoteMint: Address;
  baseTokenProgram: Address;
  quoteTokenProgram: Address;
  side: SwapSide;
};

/**
 * Read a mint account's owning token program (classic SPL Token or Token-2022).
 * Throws when the mint account doesn't exist on this cluster.
 */
async function fetchTokenProgram(
  rpc: AccountRpc,
  mint: Address,
  config?: FetchAccountConfig,
): Promise<Address> {
  const account = await fetchEncodedAccount(rpc, mint, config);
  if (!account.exists) {
    throw new Error(`Mint account ${mint} does not exist on this cluster`);
  }
  return account.programAddress;
}

/**
 * Resolve the eCLOB route for a from→to pair by finding whichever market
 * orientation exists on-chain.
 *
 * Returns `null` when neither orientation has a market — i.e. there is no
 * Dropset market for this pair on this cluster. That is an ordinary outcome,
 * not an error: on a cluster where we have not deployed the pair, an
 * aggregator-inclusive router simply drops the eCLOB candidate.
 *
 * Costs at most two `getAccountInfo` calls for market discovery (one per
 * orientation, short-circuiting on the first hit), plus one per token program
 * not supplied by the caller.
 */
export async function resolveEclobRoute(
  rpc: AccountRpc,
  input: EclobRouteInput,
  config?: FetchAccountConfig,
): Promise<EclobRoute | null> {
  const { inputMint, outputMint } = input;
  if (inputMint === outputMint) return null;

  const [inputTokenProgram, outputTokenProgram] = await Promise.all([
    input.inputTokenProgram ?? fetchTokenProgram(rpc, inputMint, config),
    input.outputTokenProgram ?? fetchTokenProgram(rpc, outputMint, config),
  ]);

  const orientations: Orientation[] = [
    {
      baseMint: inputMint,
      quoteMint: outputMint,
      baseTokenProgram: inputTokenProgram,
      quoteTokenProgram: outputTokenProgram,
      side: 'sell',
    },
    {
      baseMint: outputMint,
      quoteMint: inputMint,
      baseTokenProgram: outputTokenProgram,
      quoteTokenProgram: inputTokenProgram,
      side: 'buy',
    },
  ];

  for (const c of orientations) {
    const [market] = await findMarketPda({
      baseMint: c.baseMint,
      quoteMint: c.quoteMint,
    });
    const account = await fetchEncodedAccount(rpc, market, config);
    if (!account.exists) continue;
    const marketData = new Uint8Array(account.data);
    return {
      market,
      marketData,
      baseMint: c.baseMint,
      quoteMint: c.quoteMint,
      baseTokenProgram: c.baseTokenProgram,
      quoteTokenProgram: c.quoteTokenProgram,
      side: c.side,
      limitPriceBits: c.side === 'buy' ? PRICE_INFINITY : PRICE_ZERO,
      outputMint: c.side === 'buy' ? c.baseMint : c.quoteMint,
      outputTokenProgram:
        c.side === 'buy' ? c.baseTokenProgram : c.quoteTokenProgram,
      // Header-only decode: the ceiling is one scalar, and this runs on the
      // quote timer, so decoding the whole slab tail here would be waste. (The
      // generated `decodeMarketHeader` wants a fetched `EncodedAccount`; we
      // already hold the raw bytes, so slice and decode directly.)
      maxPlatformFeeBps: getMarketHeaderDecoder().decode(
        marketData.subarray(0, getMarketHeaderSize()),
      ).maxPlatformFee,
    };
  }
  return null;
}
