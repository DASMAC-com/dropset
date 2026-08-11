"use client";

import {
  type EclobRoute,
  platformFeeBpsFor,
  resolveEclobRoute as sdkResolveEclobRoute,
} from "@dropset/sdk";
import type { SolanaClientRuntime } from "@solana/client";
import { address } from "@solana/kit";
import {
  onchainMint,
  onchainTokenProgram,
  PROGRAM_FOR_KIND,
  stablecoinByMint,
} from "../data/currencies";
import { CLUSTER } from "../env";

type Rpc = SolanaClientRuntime["rpc"];

// Resolved routes, keyed by cluster + ordered pair. A market's existence and
// orientation are fixed for a page's lifetime — deploying one is an admin
// action — so a positive resolution stays valid indefinitely. Caching here
// rather than in any one caller is what makes it count: resolveEclobRoute sits
// on two timers (useEclobQuote re-resolves on every quote refresh,
// useOrderBook on every book poll) and each resolution costs two
// getAccountInfo reads for an answer that cannot have changed.
//
// Only *positive* resolutions are cached. A null — "no market for this pair" —
// is deliberately left uncached: on localnet the frontend serves before the TUI
// bootstrap creates the markets, so a null is routinely a not-yet rather than a
// never, and persisting it would latch the pair unavailable for the rest of the
// page's life. useEclobAvailable owns the policy for negatives, together with
// the retry that recovers from a transient one.
const routeCache = new Map<string, EclobRoute>();
// In-flight dedup, for nulls as well as hits: on a pair change the quote timer,
// the order-book poll, and the availability probe all resolve the same pair in
// the same tick, and without this they fan out into three identical lookups.
// Transient by construction — the entry is dropped once the lookup settles.
const inFlight = new Map<string, Promise<EclobRoute | null>>();

const cacheKey = (fromMint: string, toMint: string): string =>
  `${CLUSTER}:${fromMint}→${toMint}`;

// The route shape and the market-ceiling clamp both live in the SDK now — the
// route because the router owns resolution, and the clamp because it reads a
// field of that route. Re-exported here so app-side callers keep one import
// site for "the eCLOB route and what it permits".
export type { EclobRoute };
export { platformFeeBpsFor };

// Resolve the eCLOB route for a from→to pair. The market-orientation search
// itself lives in the SDK (`@dropset/sdk` → resolveEclobRoute); what stays here
// is the app-specific part the SDK can't know: the supported-stablecoin gate,
// and the last-moment substitution of the mock demo mints on localnet.
//
// The SDK takes mints in *on-chain* terms, while callers pass the real
// (display) mints — so `onchainMint` / `onchainTokenProgram` translate at this
// boundary. Both are the identity on mainnet. Passing the token programs
// explicitly also spares the SDK two mint-account reads per call, which
// matters on the quote timer.
//
// Returns null when either token isn't a supported stablecoin, the pair is
// degenerate, or no market exists for it on this cluster. A resolved route is
// memoized per cluster+pair and concurrent lookups are deduped — see routeCache
// above for why only positives persist.
export async function resolveEclobRoute(
  rpc: Rpc,
  fromMint: string,
  toMint: string,
): Promise<EclobRoute | null> {
  if (!fromMint || !toMint || fromMint === toMint) return null;
  if (!stablecoinByMint(fromMint) || !stablecoinByMint(toMint)) return null;

  const key = cacheKey(fromMint, toMint);
  const cached = routeCache.get(key);
  if (cached) return cached;
  const pending = inFlight.get(key);
  if (pending) return pending;

  const promise = sdkResolveEclobRoute(
    rpc,
    {
      inputMint: address(onchainMint(fromMint)),
      outputMint: address(onchainMint(toMint)),
      inputTokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(fromMint)],
      outputTokenProgram: PROGRAM_FOR_KIND[onchainTokenProgram(toMint)],
    },
    { commitment: "confirmed" },
  )
    .then((route) => {
      if (route) routeCache.set(key, route);
      return route;
    })
    // A rejection propagates to every caller sharing this lookup — callers that
    // treat a failure as "not yet" (the availability probe) keep doing so — but
    // it must not leave a poisoned entry behind, hence the unconditional drop.
    .finally(() => {
      if (inFlight.get(key) === promise) inFlight.delete(key);
    });
  inFlight.set(key, promise);
  return promise;
}
