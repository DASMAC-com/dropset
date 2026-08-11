"use client";

import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { ECLOB_AVAILABILITY_RETRY_MS } from "../data/timings";
import { resolveEclobRoute } from "../eclob/route";
import { IS_LOCALNET } from "../env";

export type EclobAvailability = "unknown" | "available" | "unavailable";

// pair → resolved availability. On a deployed cluster whether a market exists
// doesn't change over a page's lifetime (deploying one is an admin action), so
// the answer is cached and shared: the swap panel and the route toggle both ask
// about the current pair, and without this they'd each pay the lookup.
//
// That premise does NOT hold on localnet, where the markets are created by the
// TUI bootstrap — routinely *after* the dev server is already serving and the
// auto-opened browser has probed. A miss there is a not-yet rather than a never,
// so negatives are left uncached on localnet and re-probed (see `probe` below).
// Caching one would strand every consumer that keys off "available" for the rest
// of the page's life, including the swap panel's eCLOB quote.
const cache = new Map<string, EclobAvailability>();
// Per-pair dedupe so two mounts in the same tick fan into one lookup.
const inFlight = new Map<string, Promise<EclobAvailability>>();

const keyOf = (fromMint: string, toMint: string) => `${fromMint}→${toMint}`;

// Whether an eCLOB market exists on the current cluster for the given pair
// (in either orientation). Resolves via resolveEclobRoute — which checks the
// market account on-chain — and re-checks whenever the pair changes.
// "unknown" until the first check lands, so callers can avoid flashing an
// "unavailable" state while the lookup is in flight. Callers may gate real
// behavior on "available" (the swap panel gates its eCLOB quote on it), so a
// miss must not be sticky where a market can still show up — hence the
// localnet carve-out above.
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
            // A hit is always final. A miss is only final where deploying a
            // market is an admin action — not on localnet, where the bootstrap
            // creates them after the page is already up, so leave it uncached
            // there and let the retry below pick the market up when it lands.
            if (result === "available" || !IS_LOCALNET) {
              cache.set(key, result);
            }
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
        // Retry whatever wasn't cached above: a failed probe anywhere, and a
        // miss on localnet (where the market may still be about to appear).
        // A cached answer ends the chain, so this can't spin on a real miss
        // against a deployed cluster.
        if (result === "unknown" || (result === "unavailable" && IS_LOCALNET)) {
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
