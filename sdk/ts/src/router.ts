/**
 * The Dropset router — one entrypoint that prices a swap across our own book
 * and an aggregator, and picks whichever is better for the user.
 *
 * Two call styles:
 *
 *   - **core-style** ({@link quoteEclob}) — our venue only. Resolves the
 *     market, simulates the take with the exact on-chain matching math, and
 *     hands back everything the swap instruction needs.
 *   - **router** ({@link quoteBestRoute}) — aggregator-inclusive. Prices both
 *     legs concurrently and returns the winner plus what happened to the
 *     loser, so a caller can show *why* it routed where it did.
 *
 * The point is that a consumer calls Dropset alone and still gets the best
 * available route: the aggregator is optional, and our own pools are
 * first-class rather than something bolted on behind a third-party SDK.
 *
 * ## Comparing the two legs fairly
 *
 * Both `outAmount`s are **net** — what actually lands in the taker's account.
 * The eCLOB simulation subtracts the on-chain taker fee *and* the platform fee
 * it was quoted with, and the aggregator quote is fetched with its platform fee
 * declared (see {@link resolvePlatformFee}) so DFlow prices it in.
 * The two platform fees need not be equal — ours is clamped to the market's
 * on-chain ceiling while DFlow's is not — but each leg's figure is what that
 * route would deliver, which is what makes them comparable.
 *
 * An eCLOB quote is eligible only when it consumes the **whole** input —
 * a partial fill would leave the user holding unspent input, which is not the
 * same trade as an aggregator quote that spends all of it, so its raw output
 * is not comparable and it does not compete.
 *
 * Callers must ensure both legs denominate output in the same units. They do
 * on any single cluster; a localnet deployment quoting mock mints on-chain
 * cannot meaningfully compare against an aggregator pricing the real ones, so
 * it should pass `aggregator: null` and run our venue alone.
 */

import type { Address } from '@solana/kit';
import {
  type DflowQuote,
  type DflowQuoteInput,
  fetchDflowQuote,
  type PlatformFeeConfig,
  type ResolvedPlatformFee,
  resolvePlatformFee,
} from './dflow';
import {
  type AccountRpc,
  type EclobRoute,
  type EclobRouteInput,
  platformFeeBpsFor,
  resolveEclobRoute,
} from './route';
import type { SlotRpc } from './market';
import { initSimulator, simulateSwap } from './simulate';

/** Which venue a quote came from. */
export type RouterVenue = 'dropset' | 'dflow';

/** A quote against our own book, with the route the swap instruction needs. */
export type EclobQuote = {
  venue: 'dropset';
  inAmount: bigint;
  /** Net of **both** the on-chain taker fee and the declared platform fee. */
  outAmount: bigint;
  /** Taker fee atoms retained in the matched vaults. */
  feeAmount: bigint;
  /** Platform-fee atoms paid through to the integrator; zero when none declared. */
  platformFeeAmount: bigint;
  /**
   * The platform-fee rate this quote was computed with — the configured rate
   * clamped to the market ceiling, which is what the swap will actually charge.
   */
  platformFeeBps: number;
  /** Resting levels the take crossed. */
  legs: number;
  route: EclobRoute;
};

/** A quote from the aggregator. */
export type AggregatorQuote = {
  venue: 'dflow';
  inAmount: bigint;
  /** Net of any declared platform fee. */
  outAmount: bigint;
  priceImpactPct: string | null;
  slippageBps: number | null;
  platformFee: DflowQuote['platformFee'];
};

export type RouterQuote = EclobQuote | AggregatorQuote;

/**
 * What became of one candidate leg.
 *
 *   - `quoted` — priced, and eligible to win.
 *   - `partial` — our book priced it but could not fill the whole input, so it
 *     is reported (`quote` is set) but does not compete.
 *   - `unavailable` — not attempted, or no market exists for the pair.
 *   - `failed` — the attempt errored.
 */
export type CandidateStatus = 'quoted' | 'partial' | 'unavailable' | 'failed';

export type Candidate<Q> = {
  status: CandidateStatus;
  quote: Q | null;
  /** Why it did not win, when it didn't. */
  reason: string | null;
  /**
   * The error behind a `failed` status, unwrapped — e.g. a `DflowError` (from
   * `./dflow`), whose `kind` tells a polling caller whether to back off (a
   * rate limit), stop (a rejected pair), or retry (a transport blip).
   */
  cause?: unknown;
};

export type BestRoute = {
  best: RouterQuote;
  eclob: Candidate<EclobQuote>;
  aggregator: Candidate<AggregatorQuote>;
};

/** No leg produced a usable quote. Carries each leg's reason for diagnosis. */
export class NoRouteError extends Error {
  readonly eclob: Candidate<EclobQuote>;
  readonly aggregator: Candidate<AggregatorQuote>;
  constructor(
    eclob: Candidate<EclobQuote>,
    aggregator: Candidate<AggregatorQuote>,
  ) {
    const reasons = [
      eclob.reason ? `Dropset: ${eclob.reason}` : null,
      aggregator.reason ? `DFlow: ${aggregator.reason}` : null,
    ].filter((r): r is string => r !== null);
    super(
      reasons.length > 0
        ? `No route available (${reasons.join('; ')})`
        : 'No route available',
    );
    this.name = 'NoRouteError';
    this.eclob = eclob;
    this.aggregator = aggregator;
  }
}

/**
 * The eCLOB leg: either a route already resolved by the caller (skipping
 * discovery — worth doing when quoting on a timer) or the mint pair to
 * resolve one from.
 */
export type EclobLeg = { route: EclobRoute } | EclobRouteInput;

const hasResolvedRoute = (leg: EclobLeg): leg is { route: EclobRoute } =>
  'route' in leg;

/** The aggregator leg. Amount and abort signal come from the router call. */
export type AggregatorLeg = Omit<
  DflowQuoteInput,
  'amount' | 'signal' | 'platformFee'
> & {
  /**
   * Either an already-resolved fee (from a caller-side cache) or the raw
   * config plus the **on-chain** mint to derive the fee ATA against — which
   * the router resolves, checking that ATA exists before declaring anything.
   * The mint is explicit because it lives in a different namespace from
   * `outputMint` above: that one is the canonical mint the aggregator prices,
   * this one is whatever exists on the cluster we are checking. Omit or pass
   * `null` to charge no platform fee.
   */
  platformFee?: ResolvedPlatformFee | UnresolvedPlatformFee | null;
};

/**
 * A fee config plus the on-chain mint whose fee ATA must be checked. Supply
 * `tokenProgram` when the caller already knows it — otherwise the router reads
 * the mint account to find it, which on a quote timer is a wasted round-trip
 * per tick. Better still, resolve the fee once and pass a
 * {@link ResolvedPlatformFee}.
 */
export type UnresolvedPlatformFee = PlatformFeeConfig & {
  mint: Address;
  tokenProgram?: Address;
};

const isResolvedFee = (
  fee: ResolvedPlatformFee | UnresolvedPlatformFee,
): fee is ResolvedPlatformFee => 'feeAccount' in fee;

const errorMessage = (e: unknown): string =>
  e instanceof Error ? e.message : String(e);

/**
 * Quote a take against our own book — the core-style, eCLOB-only path.
 *
 * Resolves the market (unless the caller passed a route), reads the current
 * slot if not supplied, and simulates with the WASM binding, which runs the
 * same matching math as the program. Returns `null` when no market exists for
 * the pair on this cluster.
 *
 * `platformFeeBps` is the integrator fee the caller intends to declare on the
 * `swap` instruction; it is clamped to the market's ceiling here (see
 * {@link platformFeeBpsFor}) so an over-configured rate under-charges
 * rather than returning an all-zero quote. Quoting with the fee the executor
 * will actually declare is what keeps the displayed output equal to what lands
 * in the taker's account.
 */
export async function quoteEclob(
  rpc: AccountRpc & SlotRpc,
  input: {
    leg: EclobLeg;
    amount: bigint;
    /** Current slot for the slot half of level-expiry filtering; read via
     * `getSlot` when omitted. */
    nowSlot?: number;
    /**
     * Wall-clock unix seconds for the wall half. **Required** — the engine
     * judges this deadline against cluster time, so a caller that cannot
     * bound its own clock (a browser) must pass a chain-read time. See
     * `nowUnix()` in `./market`, which remains available to hosts that can.
     */
    nowUnix: number;
    /** Configured integrator fee in bps; clamped to the market ceiling. */
    platformFeeBps?: number;
  },
): Promise<EclobQuote | null> {
  const route = hasResolvedRoute(input.leg)
    ? input.leg.route
    : await resolveEclobRoute(rpc, input.leg);
  if (!route) return null;

  const platformFeeBps = platformFeeBpsFor(route, input.platformFeeBps ?? 0);
  const resolvedNowSlot = input.nowSlot ?? Number(await rpc.getSlot().send());
  await initSimulator();
  const q = simulateSwap(
    route.marketData,
    route.side,
    input.amount,
    route.limitPriceBits,
    resolvedNowSlot,
    input.nowUnix,
    platformFeeBps,
  );
  return {
    venue: 'dropset',
    inAmount: q.inAmount,
    outAmount: q.outAmount,
    feeAmount: q.feeAmount,
    platformFeeAmount: q.platformFeeAmount,
    platformFeeBps,
    legs: q.legs,
    route,
  };
}

/**
 * Decide whether an eCLOB quote is eligible to compete for best route.
 *
 * The eligibility rule, in one place: a quote must exist, produce output, and
 * consume the **whole** requested input. A partial fill is reported (`partial`,
 * with the quote attached) but never competes — see the module docs for why.
 */
export function classifyEclobQuote(
  quote: EclobQuote | null,
  requestedAmount: bigint,
): Candidate<EclobQuote> {
  if (!quote) {
    return {
      status: 'unavailable',
      quote: null,
      reason: 'no Dropset market for this pair',
    };
  }
  if (quote.outAmount === 0n) {
    return { status: 'failed', quote, reason: 'no liquidity for this size' };
  }
  if (quote.inAmount !== requestedAmount) {
    return {
      status: 'partial',
      quote,
      reason: 'book too thin to fill the whole amount',
    };
  }
  return { status: 'quoted', quote, reason: null };
}

/**
 * Pick the winner from two classified candidates: the larger net `outAmount`,
 * with a tie going to our own book (same output for the user, one less hop, no
 * third-party dependency). Only `quoted` candidates compete.
 *
 * Throws {@link NoRouteError} when neither is eligible.
 */
export function selectBestRoute(
  eclob: Candidate<EclobQuote>,
  aggregator: Candidate<AggregatorQuote>,
): RouterQuote {
  const ours = eclob.status === 'quoted' ? eclob.quote : null;
  const theirs = aggregator.status === 'quoted' ? aggregator.quote : null;
  if (ours && theirs) {
    return ours.outAmount >= theirs.outAmount ? ours : theirs;
  }
  if (ours) return ours;
  if (theirs) return theirs;
  throw new NoRouteError(eclob, aggregator);
}

/**
 * An eCLOB leg paired with the clocks that scope level expiry against it.
 *
 * Level expiry is dual-domain — a level rests only inside **both** its slot
 * deadline and its wall-clock deadline — and the two halves are sourced
 * differently. `nowSlot` is a chain read, so it is optional and filled from
 * `getSlot`. `nowUnix` is not: the engine judges it against cluster time, so
 * it is required and must come from a clock the caller can actually bound.
 */
export type GatedEclobLeg = {
  leg: EclobLeg;
  /** Current slot; read via `getSlot` when omitted. */
  nowSlot?: number;
  /** Wall-clock unix seconds, from a bounded clock — never a raw device one. */
  nowUnix: number;
};

/** Price our own book, folding every failure mode into a {@link Candidate}. */
async function eclobCandidate(
  rpc: AccountRpc & SlotRpc,
  gated: GatedEclobLeg | null,
  amount: bigint,
  platformFeeBps: number | undefined,
): Promise<Candidate<EclobQuote>> {
  if (!gated) {
    return { status: 'unavailable', quote: null, reason: 'not requested' };
  }
  try {
    return classifyEclobQuote(
      await quoteEclob(rpc, {
        leg: gated.leg,
        amount,
        nowSlot: gated.nowSlot,
        nowUnix: gated.nowUnix,
        platformFeeBps,
      }),
      amount,
    );
  } catch (e) {
    return { status: 'failed', quote: null, reason: errorMessage(e), cause: e };
  }
}

/** Price the aggregator, resolving the fee guard first. */
async function aggregatorCandidate(
  rpc: AccountRpc,
  leg: AggregatorLeg | null,
  amount: bigint,
  signal: AbortSignal | undefined,
): Promise<Candidate<AggregatorQuote>> {
  if (!leg) {
    return { status: 'unavailable', quote: null, reason: 'not requested' };
  }
  try {
    const configured = leg.platformFee ?? null;
    const platformFee =
      configured === null
        ? null
        : isResolvedFee(configured)
          ? configured
          : await resolvePlatformFee(rpc, {
              fee: configured,
              mint: configured.mint,
              tokenProgram: configured.tokenProgram,
            });

    const q = await fetchDflowQuote({ ...leg, amount, platformFee, signal });
    if (q.outAmount === 0n) {
      return {
        status: 'failed',
        quote: null,
        reason: 'aggregator returned no output',
      };
    }
    const quote: AggregatorQuote = {
      venue: 'dflow',
      inAmount: q.inAmount,
      outAmount: q.outAmount,
      priceImpactPct: q.priceImpactPct,
      slippageBps: q.slippageBps,
      platformFee: q.platformFee,
    };
    if (q.inAmount !== amount) {
      // The same rule the eCLOB leg is held to: a quote that doesn't spend the
      // whole input is a different trade, so its output isn't comparable. DFlow
      // documents `inAmount` as the *maximum* input, and echoes the requested
      // amount in practice — so this is a guard on the contract, not a case we
      // expect. Reported, but it does not compete.
      return {
        status: 'partial',
        quote,
        reason: 'aggregator would not spend the whole amount',
      };
    }
    return { status: 'quoted', quote, reason: null };
  } catch (e) {
    if (e instanceof DOMException && e.name === 'AbortError') throw e;
    return { status: 'failed', quote: null, reason: errorMessage(e), cause: e };
  }
}

/**
 * Price both legs concurrently and return the better one.
 *
 * The winner is simply the larger net `outAmount`. A tie goes to our own book:
 * the user gets the same output either way, with one less hop and no
 * dependency on a third party. Pass `eclob: null` or `aggregator: null` to run
 * a single leg.
 *
 * Throws {@link NoRouteError} when neither leg produced an eligible quote; the
 * error carries both candidates so a caller can report the real reason.
 */
export async function quoteBestRoute(
  rpc: AccountRpc & SlotRpc,
  input: {
    amount: bigint;
    /**
     * Our own leg together with the clocks that gate its book, or `null` to
     * price the aggregator alone. The clocks are nested here, rather than
     * sitting beside `amount`, so that `nowUnix` is required exactly when
     * there is a book to gate and absent when there is not — a flat field
     * could not express that. See {@link GatedEclobLeg} for why the wall
     * clock is the required half.
     */
    eclob: GatedEclobLeg | null;
    aggregator: AggregatorLeg | null;
    signal?: AbortSignal;
    /**
     * Configured integrator fee for the **eCLOB** leg, in bps — clamped to the
     * market ceiling. The aggregator leg carries its own fee on
     * `aggregator.platformFee`, since the two are settled by different parties
     * against different constraints.
     */
    platformFeeBps?: number;
  },
): Promise<BestRoute> {
  const { amount, signal, platformFeeBps } = input;
  const [eclob, aggregator] = await Promise.all([
    eclobCandidate(rpc, input.eclob, amount, platformFeeBps),
    aggregatorCandidate(rpc, input.aggregator, amount, signal),
  ]);
  return { best: selectBestRoute(eclob, aggregator), eclob, aggregator };
}
