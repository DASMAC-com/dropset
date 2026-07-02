"use client";

import { useEffect } from "react";
import { stablecoinMint } from "@/lib/data/currencies";
import { IS_LOCALNET } from "@/lib/env";
import { useEclobAvailable } from "@/lib/hooks/useEclobAvailable";
import { useSwapStore } from "@/lib/store";

// Compact "Dropset route only" switch for the swap details row. Off routes via
// the DFlow aggregator (best route); on routes directly through our own
// market. It renders only when an eCLOB market actually exists for the current
// pair on this cluster — so it never offers a route that isn't there. On
// localnet it's forced on and disabled (the store also clamps routeMode, so
// this is the affordance, not the enforcement).
export function RouteModeToggle() {
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const routeMode = useSwapStore((s) => s.routeMode);
  const setRouteMode = useSwapStore((s) => s.setRouteMode);

  const availability = useEclobAvailable(
    stablecoinMint(fromStablecoin),
    stablecoinMint(toStablecoin),
  );
  const on = routeMode === "eclob";

  // Fall back to best-route when the pair has no eCLOB market — e.g. after a
  // token or direction change to an unsupported pair — so the eCLOB route
  // never sits selected against a market that doesn't exist. Localnet is
  // forced-eCLOB and exempt.
  useEffect(() => {
    if (!IS_LOCALNET && on && availability === "unavailable") {
      setRouteMode("best");
    }
  }, [on, availability, setRouteMode]);

  // Only offer the switch once a market is confirmed for this pair.
  if (availability !== "available") return null;

  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label="Dropset route only"
      disabled={IS_LOCALNET}
      onClick={() => setRouteMode(on ? "best" : "eclob")}
      title={
        IS_LOCALNET
          ? "Localnet routes through Dropset only"
          : "Route directly through the Dropset order book (no aggregator)"
      }
      className="flex shrink-0 items-center gap-1.5 text-muted-fg transition-colors hover:text-foreground disabled:cursor-not-allowed disabled:opacity-60 disabled:hover:text-muted-fg"
    >
      <span>Dropset route only</span>
      <span
        className={`relative h-4 w-7 rounded-full transition-colors ${
          on ? "bg-accent-buy" : "bg-border"
        }`}
      >
        <span
          className={`absolute top-0.5 left-0.5 h-3 w-3 rounded-full bg-background transition-transform ${
            on ? "translate-x-3" : "translate-x-0"
          }`}
        />
      </span>
    </button>
  );
}
