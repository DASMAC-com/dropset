"use client";

import { useState } from "react";
import { tokenIconFallbackUrl, tokenIconUrl } from "@/lib/data/currencies";

// Which URL to render, given the two candidates and the one that has already
// failed. Split out of the component because it is the whole of the fallback
// logic and it is pure — inlined in the JSX it would be unreachable by the
// unit runner, which has no DOM. See TokenIcon.test.ts.
//
// `failedSrc` holds a URL rather than a boolean so the state resets by itself
// when the symbol changes: the new symbol's `primary` differs, so a stale
// failure cannot latch onto it. That matters in reused list rows and in the
// picker trigger, which swap symbols in place.
export function resolveIconSrc(
  primary: string,
  fallback: string,
  failedSrc: string | null,
): string {
  return failedSrc === primary && fallback ? fallback : primary;
}

// A stablecoin logo, sourced from the build-time mirror on our own origin
// with a one-shot fallback to the issuer's canonical URL.
//
// The fallback is the point of the component. The mirror manifest can name a
// file that is missing or unreadable — a partial deploy, an asset removed
// after the build — and a bare <img> pointed at it renders nothing, with no
// signal that anything is wrong. Every call site used to hand-roll identical
// <img> markup with no error handler, so there was no single place to put the
// recovery. There is now.
//
// Accepted tradeoff: the fallback URL is off-origin, so an icon that fails
// re-introduces exactly the third-party request the mirror exists to avoid.
// That is confined to the error path, and a visible logo is worth more than
// the request it costs. Note it before adding a CSP — an `img-src 'self'`
// would silently make this recovery dead code.
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
  const [failedSrc, setFailedSrc] = useState<string | null>(null);

  const primary = tokenIconUrl(symbol);
  const fallback = tokenIconFallbackUrl(symbol);
  const src = resolveIconSrc(primary, fallback, failedSrc);

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
      // Only the primary promotes, and only when there is somewhere to go.
      // Recording a fallback failure too would flip `src` back to the primary
      // on the next render and loop between two dead URLs forever; the
      // `fallback` test keeps a symbol with no mirror from setting state that
      // cannot change anything.
      onError={() => {
        if (src === primary && fallback) setFailedSrc(primary);
      }}
    />
  );
}
