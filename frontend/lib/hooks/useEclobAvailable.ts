"use client";

import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { resolveEclobRoute } from "../eclob/route";

export type EclobAvailability = "unknown" | "available" | "unavailable";

// pair → resolved availability. Whether a market exists for a pair doesn't
// change over a page's lifetime (deploying one is an admin action), so the
// answer is cached and shared: the swap panel and the route toggle both ask
// about the current pair, and without this they'd each pay the lookup.
const cache = new Map<string, EclobAvailability>();
// Per-pair dedupe so two mounts in the same tick fan into one lookup.
const inFlight = new Map<string, Promise<EclobAvailability>>();

const keyOf = (fromMint: string, toMint: string) => `${fromMint}→${toMint}`;

// Whether an eCLOB market exists on the current cluster for the given pair
// (in either orientation). Resolves via resolveEclobRoute — which checks the
// market account on-chain — and re-checks whenever the pair changes.
// "unknown" until the first check lands, so callers can avoid flashing an
// "unavailable" state while the lookup is in flight.
export function useEclobAvailable(
  fromMint: string,
  toMint: string,
): EclobAvailability {
  const client = useSolanaClient();
  const key = keyOf(fromMint, toMint);
  const [state, setState] = useState<EclobAvailability>(
    () => cache.get(key) ?? "unknown",
  );

  useEffect(() => {
    let cancelled = false;
    const cached = cache.get(key);
    if (cached) {
      setState(cached);
      return;
    }
    setState("unknown");

    let pending = inFlight.get(key);
    if (!pending) {
      pending = resolveEclobRoute(client.runtime.rpc, fromMint, toMint)
        .then((route): EclobAvailability => {
          const result = route ? "available" : "unavailable";
          // Only a definitive answer is cached. A failed lookup falls through
          // to the catch below and is left uncached, so re-selecting the pair
          // retries instead of sticking on a transient RPC error.
          cache.set(key, result);
          return result;
        })
        .catch((): EclobAvailability => "unavailable")
        .finally(() => {
          inFlight.delete(key);
        });
      inFlight.set(key, pending);
    }
    pending.then((result) => {
      if (!cancelled) setState(result);
    });

    return () => {
      cancelled = true;
    };
  }, [client, fromMint, toMint, key]);

  return state;
}
