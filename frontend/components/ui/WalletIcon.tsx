"use client";

import Image from "next/image";
import { useState } from "react";

// Which URL to render, given the two candidates and the ones that have already
// failed — or null when both are spent and the caller's terminal placeholder
// should take over. Split out of the component because it is the whole of the
// fallback logic and it is pure; the unit runner has no DOM and could not
// otherwise reach any of it. See WalletIcon.test.ts.
//
// `failed` holds URLs rather than a count so the state resets by itself when
// the wallet changes: the new wallet's candidates differ, so a stale failure
// cannot latch onto them. That matters because the picker rebuilds its rows
// every time a connector is discovered, swapping icons in place.
//
// Unlike TokenIcon's two-state resolver, this one is allowed to run out. A
// wallet has a meaningful terminal state — the letter avatar — so recording
// the second failure buys something real here, where for a token icon it would
// only trade one empty box for another. The list is finite and each error adds
// to it, so the promotion still cannot loop.
export function resolveWalletIconSrc(
  primary: string,
  fallback: string,
  failed: readonly string[],
): string | null {
  for (const candidate of [primary, fallback]) {
    if (candidate && !failed.includes(candidate)) return candidate;
  }
  return null;
}

// A wallet's brand logo, sourced from the build-time mirror on our own origin
// with a one-shot fallback to the vendor's canonical URL and a letter avatar
// behind that.
//
// The fallback is the point of the component. The mirror manifest can name a
// file that is missing or unreadable — a partial deploy, an asset pruned after
// the build — and the picker's `w.icon ? <Image/> : <avatar/>` test could not
// see it: a dead mirror path is truthy, so the avatar branch never fired and
// the row rendered an empty 32px box. Now the failure is observed at render
// time, where it actually happens, and every wallet ends up showing something.
//
// Accepted tradeoff: the fallback URL is off-origin, so an icon that fails
// re-introduces exactly the third-party request the mirror exists to avoid.
// That is confined to the error path, and a visible logo is worth more than
// the request it costs. Note it before adding a CSP — an `img-src 'self'`
// would silently make this recovery dead code.
export function WalletIcon({
  // Only ever read for the avatar's initial, so the component never needs the
  // whole PickerWallet.
  name,
  // Pre-resolved by the caller, where TokenIcon takes a symbol and does its
  // own lookup. It has to be: a live Wallet Standard connector can override
  // the curated icon with its own data URI, and only `buildPickerWallets`
  // knows whether one did.
  src,
  fallbackSrc,
  size,
  className,
}: {
  name: string;
  src?: string;
  fallbackSrc?: string;
  size: number;
  className?: string;
}) {
  const [failed, setFailed] = useState<readonly string[]>([]);

  const resolved = resolveWalletIconSrc(src ?? "", fallbackSrc ?? "", failed);

  // No icon at all, or every source spent: fall back to the wallet's initial.
  if (!resolved) {
    return (
      <div
        // Decorative: the row already renders the wallet's name as text, so
        // without this a screen reader announces the bare initial in front of
        // it ("P Phantom"). Matches TokenIcon's placeholder.
        aria-hidden
        className={`flex items-center justify-center bg-muted font-bold text-muted-fg text-xs ${className ?? ""}`}
        // Explicit dimensions because `className` is the caller's to choose and
        // may not carry any — the avatar should reserve the same box as the
        // image it stands in for regardless.
        style={{ width: size, height: size }}
      >
        {name.charAt(0)}
      </div>
    );
  }

  return (
    <Image
      src={resolved}
      alt=""
      aria-hidden
      width={size}
      height={size}
      className={className}
      // The sources are a local mirror, an arbitrary vendor URL, and inline
      // connector data URIs — none of which the optimizer is configured for.
      unoptimized
      onError={() =>
        setFailed((prev) =>
          prev.includes(resolved) ? prev : [...prev, resolved],
        )
      }
    />
  );
}
