"use client";

import { type Stablecoin, tokenIconUrl } from "@/lib/currencies";
import { useLiquidityLookup } from "@/lib/useUsdQuote";

// Shared stablecoin-row identity block used by the dropdown TokenPicker and
// the globe's country-anchored picker. Renders icon + symbol (with optional
// "Illiquid" badge when Jupiter has no reliable USD price) + secondary name.
// Returns a fragment so each parent supplies its own flex container and
// action buttons.
export function StableTokenIdentity({
  s,
  symbolClassName,
}: {
  s: Stablecoin;
  symbolClassName?: string;
}) {
  const liquidityOf = useLiquidityLookup();
  const illiquid = liquidityOf(s.mint) === "illiquid";
  return (
    <>
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
      <img
        src={tokenIconUrl(s.symbol)}
        alt=""
        aria-hidden
        width={20}
        height={20}
        className="h-5 w-5 shrink-0 rounded-full"
      />
      <span className="flex min-w-0 flex-1 flex-col text-sm">
        <span className="flex items-center gap-1.5">
          <span className={`font-mono ${symbolClassName ?? ""}`}>
            {s.symbol}
          </span>
          {illiquid && (
            <span
              className="rounded border border-border bg-background px-1.5 py-0.5 text-[10px] text-muted-fg uppercase tracking-wide"
              title="No reliable USD price from Jupiter — token may be illiquid or low-volume."
            >
              Illiquid
            </span>
          )}
        </span>
        {s.name !== s.symbol && (
          <span className="truncate text-muted-fg text-xs">{s.name}</span>
        )}
      </span>
    </>
  );
}
