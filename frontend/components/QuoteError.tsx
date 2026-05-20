"use client";

import type { DflowQuote } from "@/lib/useDflowQuote";

// Inline message shown under the swap panel when DFlow declines a quote
// (most commonly `route_not_found` when the input amount exceeds available
// liquidity). Renders nothing for ok/loading/idle/skipped/rateLimited —
// rate-limit pauses have their own banner, and skipped/idle are silent
// by design.
export function QuoteError({ quote }: { quote: DflowQuote }) {
  if (quote.status !== "error" || !quote.error) return null;
  // Make the "route_not_found" copy human — the API code is accurate but
  // unhelpful to a user staring at a swap panel.
  const friendly =
    quote.error === "Route not found"
      ? "No route for this amount — try a smaller size."
      : quote.error;
  return (
    <div className="rounded-lg border border-border bg-muted px-3 py-2 text-muted-fg text-sm">
      {friendly}
    </div>
  );
}
