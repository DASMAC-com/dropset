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

type Rpc = SolanaClientRuntime["rpc"];

// Coalesces concurrent resolutions of the same pair. On a pair change the quote
// timer, the order-book poll, and the availability probe all resolve the same
// pair in the same tick; without this they fan out into three identical
// lookups. Transient by construction — the entry is dropped as soon as the
// lookup settles, so each tick still reads the market account fresh.
//
// Deliberately *not* a persistent cache, though the pair→market mapping looks
// eminently cacheable. An `EclobRoute` carries `marketData`: the market
// account's raw bytes — the live order book — captured at resolve time, and
// both the quote (`quoteEclob` → `simulateSwap`) and the swap builder price
// off exactly those bytes. Memoizing a route beyond its own tick would freeze
// the book: `useEclobQuote` would re-simulate identical bytes forever while
// `currentSlot` advanced out from under them (so levels would expire and the
// quote would decay to "no liquidity"), and `eclobSwap`'s `minOut` would be
// sized against page-load-time depth instead of current depth — a slippage
// floor that silently loosens the longer a tab stays open.
//
// Only the route's *identity* (market, orientation, mints, token programs) is
// durable. Caching just that would still have to fetch the account for fresh
// bytes and for `maxPlatformFeeBps`, which is header-decoded from them, so it
// would save at most one `getAccountInfo` on the reverse-orientation pair —
// not worth holding a second, partial route shape.
const inFlight = new Map<string, Promise<EclobRoute | null>>();

const dedupKey = (fromMint: string, toMint: string): string =>
  `${fromMint}→${toMint}`;

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
// degenerate, or no market exists for it on this cluster. Concurrent lookups of
// one pair are coalesced; nothing is cached across ticks — see `inFlight` above
// for why a route must not outlive its own resolution.
export async function resolveEclobRoute(
  rpc: Rpc,
  fromMint: string,
  toMint: string,
): Promise<EclobRoute | null> {
  if (!fromMint || !toMint || fromMint === toMint) return null;
  if (!stablecoinByMint(fromMint) || !stablecoinByMint(toMint)) return null;

  const key = dedupKey(fromMint, toMint);
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
    // A rejection propagates to every caller sharing this lookup — callers that
    // treat a failure as "not yet" (the availability probe) keep doing so — but
    // it must not leave a poisoned entry behind, hence the unconditional drop.
    .finally(() => {
      if (inFlight.get(key) === promise) inFlight.delete(key);
    });
  inFlight.set(key, promise);
  return promise;
}
