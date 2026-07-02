"use client";

import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { resolveEclobRoute } from "../eclob/route";

export type EclobAvailability = "unknown" | "available" | "unavailable";

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
  const [state, setState] = useState<EclobAvailability>("unknown");

  useEffect(() => {
    let cancelled = false;
    setState("unknown");
    resolveEclobRoute(client.runtime.rpc, fromMint, toMint)
      .then((route) => {
        if (!cancelled) setState(route ? "available" : "unavailable");
      })
      .catch(() => {
        if (!cancelled) setState("unavailable");
      });
    return () => {
      cancelled = true;
    };
  }, [client, fromMint, toMint]);

  return state;
}
