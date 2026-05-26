"use client";

import { useEffect, useState } from "react";
import { DFLOW_QUOTE, useRateLimit } from "@/lib/dflow/rateLimitBudget";

// Inline banner that explains a rate-limit pause to the user, with a
// live countdown to recovery. Renders nothing unless we're currently
// exhausted (`exhaustedUntil` in the future). The DFlow quote hook owns
// the auto-resume; this component is presentation only.
//
// We don't try to differentiate "too many tabs" from "shared NAT" from
// "extension noise" — the user-facing copy lists the most common cause
// without over-claiming it as the only one.
export function RateLimitMessage() {
  const snapshot = useRateLimit(DFLOW_QUOTE);
  const [now, setNow] = useState(() => Date.now());

  const active =
    snapshot?.exhaustedUntil !== null &&
    snapshot?.exhaustedUntil !== undefined &&
    snapshot.exhaustedUntil > now;

  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(id);
  }, [active]);

  if (!active || !snapshot?.exhaustedUntil) return null;
  const secondsLeft = Math.max(
    0,
    Math.ceil((snapshot.exhaustedUntil - now) / 1000),
  );

  return (
    <div
      role="status"
      aria-live="polite"
      className="rounded-lg border border-accent/40 bg-accent/10 px-3 py-2 text-accent text-sm"
    >
      <div className="font-medium">Quote refresh paused</div>
      <div className="text-foreground/80">
        Your network hit DFlow&apos;s rate limit. Retrying in{" "}
        <span className="font-mono tabular-nums">{secondsLeft}s</span>. Close
        other tabs of this app if any are open.
      </div>
    </div>
  );
}
