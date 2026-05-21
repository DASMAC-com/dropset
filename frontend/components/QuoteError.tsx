"use client";

import type { DflowQuote } from "@/lib/useDflowQuote";
import { useLiquidityLookup } from "@/lib/useUsdQuote";

// Inline message shown under the swap panel when DFlow declines a quote.
// `route_not_found` is the common case and has two distinct user-facing
// meanings that DFlow itself doesn't differentiate:
//   1. Amount-too-large — the pair is fine, the requested size exceeds what
//      DFlow's routers can fill in one transaction (e.g., 10M USDC→USDT).
//   2. Pair un-routable — one or both tokens lack the liquidity to be routed
//      at any amount (e.g., VEUR → EURCV).
// We disambiguate using Jupiter's per-token data: if both tokens have a
// usable USD reference price (`useLiquidityLookup` → "liquid"), DFlow's
// rejection is almost certainly size-related. Otherwise, default to the
// safer "no route" framing. "unknown" (prefetch hasn't completed) is
// treated as "liquid" so the panel doesn't show a misleading message during
// the brief warm-up window.
//
// Renders nothing for ok/loading/idle/skipped/rateLimited — rate-limit
// pauses have their own banner, and skipped/idle are silent by design.
export function QuoteError({
  quote,
  fromMint,
  toMint,
}: {
  quote: DflowQuote;
  fromMint: string;
  toMint: string;
}) {
  const liquidity = useLiquidityLookup();
  if (quote.status !== "error" || !quote.error) return null;
  let friendly = quote.error;
  if (quote.error === "Route not found") {
    const fromIlliquid = liquidity(fromMint) === "illiquid";
    const toIlliquid = liquidity(toMint) === "illiquid";
    friendly =
      fromIlliquid || toIlliquid
        ? "No route available for this pair right now."
        : "Amount too large to route in one swap — try a smaller size.";
  }
  return (
    <div className="rounded-lg border border-border bg-muted px-3 py-2 text-muted-fg text-sm">
      {friendly}
    </div>
  );
}
