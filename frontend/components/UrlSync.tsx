"use client";

import { usePathname } from "next/navigation";
import { useEffect } from "react";
import { resolvePair } from "@/lib/currencies";
import { useSwapStoreApi } from "@/lib/store";
import { swapHref } from "@/lib/swapUrl";

// Reconciles the swap store with the URL when /swap is the active route.
// Single source of truth: the URL. On mount, route change, and browser
// back/forward, this effect:
//   1. reads ?from / ?to from window.location.search (NOT useSearchParams,
//      which can lag behind the actual URL across router transitions),
//   2. resolves the slugs into a canonical pair via resolvePair,
//   3. updates the store via setSides if it doesn't match,
//   4. canonicalizes the URL via history.replaceState if it differs from
//      `?from=<sym>&to=<sym>` (e.g. shrinks `?to=usd` into the resolved pair).
//
// Mutation sites (token pickers, swap-direction arrow) update the URL
// directly via useSwapNav. They don't fire popstate, so this effect doesn't
// re-run from their writes — only on actual navigation events. That breaks
// the prior reader/writer ping-pong and eliminates the production-only race
// where a stale useSearchParams snapshot reverted a fresh pick.
export function UrlSync() {
  const pathname = usePathname();
  const store = useSwapStoreApi();

  useEffect(() => {
    if (pathname !== "/swap") return;

    const reconcile = () => {
      const sp = new URLSearchParams(window.location.search);
      const fromSlug = sp.get("from");
      const toSlug = sp.get("to");
      const pair = resolvePair(fromSlug, toSlug);

      const cur = store.getState();
      const storeMatches =
        pair.from.currency === cur.from.currency &&
        pair.from.stablecoin === cur.from.stablecoin &&
        pair.to.currency === cur.to.currency &&
        pair.to.stablecoin === cur.to.stablecoin;
      if (!storeMatches) cur.setSides(pair.from, pair.to);

      const canonical = swapHref(pair.from.stablecoin, pair.to.stablecoin);
      const canonicalSearch = canonical.slice(canonical.indexOf("?"));
      const currentSearch = window.location.search;
      if (currentSearch !== canonicalSearch) {
        const hash = window.location.hash;
        window.history.replaceState(null, "", `/swap${canonicalSearch}${hash}`);
      }
    };

    reconcile();
    window.addEventListener("popstate", reconcile);
    return () => window.removeEventListener("popstate", reconcile);
  }, [pathname, store]);

  return null;
}
