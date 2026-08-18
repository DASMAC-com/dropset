"use client";

import { useState } from "react";
import { tokenIconFallbackUrl, tokenIconUrl } from "@/lib/data/currencies";

// A stablecoin logo, sourced from the build-time mirror on our own origin
// with a one-shot fallback to the issuer's canonical URL.
//
// The fallback is the point of the component. The mirror manifest can name a
// file that is missing or unreadable — a partial deploy, an asset removed
// after the build — and a bare <img> pointed at it renders nothing, with no
// signal that anything is wrong. Every call site used to hand-roll identical
// <img> markup with no error handler, so there was no single place to put the
// recovery. There is now.
export function TokenIcon({
  symbol,
  size,
  className,
  // Set when the icon stands in for the symbol rather than accompanying it
  // (the vaults dialog uses it in place of the words "Base"/"Quote"), so the
  // symbol is still reachable on hover.
  title,
}: {
  symbol: string;
  size: number;
  className?: string;
  title?: string;
}) {
  // Track the URL that failed rather than a boolean, so the state resets by
  // itself when `symbol` changes (this renders in reused list rows and in the
  // picker trigger, which swaps symbols in place). A boolean would latch the
  // previous symbol's failure onto the next one.
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  const primary = tokenIconUrl(symbol);
  const fallback = tokenIconFallbackUrl(symbol);
  const src = failedSrc === primary && fallback ? fallback : primary;

  // Nothing to show for an unknown symbol. Rendering <img src=""> instead
  // would make the browser re-request the current page as the image.
  if (!src) {
    return (
      <span
        aria-hidden
        className={className}
        style={{ width: size, height: size }}
        title={title}
      />
    );
  }

  return (
    // biome-ignore lint/performance/noImgElement: small static icon, no optimization needed
    <img
      src={src}
      alt=""
      aria-hidden
      width={size}
      height={size}
      className={className}
      title={title}
      // Only the primary promotes to the fallback. Recording a fallback
      // failure too would flip `src` back to the primary on the next render
      // and loop between two dead URLs forever.
      onError={() => {
        if (src === primary) setFailedSrc(primary);
      }}
    />
  );
}
