"use client";

import { IS_LOCALNET } from "@/lib/env";
import { type RouteMode, useSwapStore } from "@/lib/store";

const OPTIONS: { mode: RouteMode; label: string; title: string }[] = [
  {
    mode: "best",
    label: "Best route",
    title:
      "Route through the DFlow aggregator for the best price across venues",
  },
  {
    mode: "eclob",
    label: "eCLOB only",
    title: "Route directly through the Dropset order book — no external venue",
  },
];

// Segmented "Best route" / "eCLOB only" selector for the swap path. On
// localnet "Best route" is disabled — the aggregator only knows mainnet
// liquidity — so the control shows eCLOB-only locked on (the store clamps it
// too, so this is a UI affordance, not the enforcement).
export function RouteModeToggle() {
  const routeMode = useSwapStore((s) => s.routeMode);
  const setRouteMode = useSwapStore((s) => s.setRouteMode);

  return (
    <div className="flex items-center gap-1">
      {OPTIONS.map((o) => {
        const active = routeMode === o.mode;
        const disabled = IS_LOCALNET && o.mode === "best";
        return (
          <button
            key={o.mode}
            type="button"
            onClick={() => setRouteMode(o.mode)}
            disabled={disabled}
            aria-pressed={active}
            title={
              disabled
                ? "Localnet routes through the Dropset eCLOB only"
                : o.title
            }
            className={`rounded border px-2 py-1 font-medium text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40 ${
              active
                ? "border-accent-buy text-accent-buy"
                : "border-border text-muted-fg hover:border-accent-buy hover:text-accent-buy"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
