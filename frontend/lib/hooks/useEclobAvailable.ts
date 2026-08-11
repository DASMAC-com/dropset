"use client";

import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { ECLOB_AVAILABILITY_RETRY_MS } from "../data/timings";
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
    let retry: number | undefined;
    const cached = cache.get(key);
    if (cached) {
      setState(cached);
      return;
    }
    setState("unknown");

    // Self-rescheduling probe. A *failed* probe retries on a timer, because
    // nothing else would: the effect only re-runs when the pair or client
    // changes, so a single early failure would latch for the page's lifetime —
    // exactly the `make demo` case, where the frontend serves before the
    // validator accepts connections. Consumers hide the route switch while this
    // is unresolved, so latching silently drops the switch (and the details
    // chevron that carries it) on a cluster that does have the pair.
    const probe = () => {
      let pending = inFlight.get(key);
      if (!pending) {
        pending = resolveEclobRoute(client.runtime.rpc, fromMint, toMint)
          .then((route): EclobAvailability => {
            const result = route ? "available" : "unavailable";
            // Only a definitive answer is cached — "no market for this pair" is
            // as final as "here it is", since deploying one is an admin action,
            // and a cached answer stops the retry below.
            cache.set(key, result);
            return result;
          })
          // A probe that *failed* is not an answer, so it stays uncached and
          // reports "unknown" rather than claiming the market doesn't exist.
          .catch((): EclobAvailability => "unknown")
          .finally(() => {
            inFlight.delete(key);
          });
        inFlight.set(key, pending);
      }
      pending.then((result) => {
        if (cancelled) return;
        setState(result);
        if (result === "unknown") {
          retry = window.setTimeout(probe, ECLOB_AVAILABILITY_RETRY_MS);
        }
      });
    };
    probe();

    return () => {
      cancelled = true;
      if (retry !== undefined) window.clearTimeout(retry);
    };
  }, [client, fromMint, toMint, key]);

  return state;
}
