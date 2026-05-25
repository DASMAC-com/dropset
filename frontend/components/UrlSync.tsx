"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect } from "react";
import { resolvePair } from "@/lib/currencies";
import { useSwapStoreApi } from "@/lib/store";
import { swapHref } from "@/lib/swapUrl";

// Reconciles the swap store with the URL when /swap is the active route.
//
// Trust direction depends on HOW we got here:
//
//   * On mount (route change to /swap, initial page load): the **store
//     wins**. Either the provider factory just seeded the store from the
//     URL on initial mount, or a mutation site (picker, swap-direction)
//     mutated the store and pushed the matching URL. Reading
//     window.location.search here is racy in production — across the
//     React Compiler + App Router transition window, the read can
//     observe the *previous* /swap URL (e.g. the canonicalized
//     ?from=EURC&to=USDC from a ?to=usd deep-link) while the just-pushed
//     URL is still settling. So we don't read it at mount; we
//     unconditionally canonicalize from the store.
//
//   * On popstate (browser back/forward): the **URL wins**. The user is
//     replaying history; the store needs to follow whatever pair the
//     history entry encodes. Read the URL, reconcile the store, then
//     canonicalize.
//
// This is the post-PF-1 design — the prior version trusted the URL on
// every mount, which was the load-bearing cause of the production
// "picker click reverts" bug, not the writer-effect race the original
// audit RCA proposed.
export function UrlSync() {
  const pathname = usePathname();
  // Subscribed only to trigger an effect re-run on same-path-different-
  // query navigation (e.g. header logo Link from /swap?from=X&to=Y to
  // bare /swap). The actual URL read still happens via window.location
  // inside the effect — useSearchParams is a re-render signal here, not
  // the source of truth.
  const searchParams = useSearchParams();
  const spString = searchParams.toString();
  const store = useSwapStoreApi();

  useEffect(() => {
    if (pathname !== "/swap") return;
    // Touch spString so biome counts it as used — same-path-different-
    // query nav (e.g. header logo Link back to bare /swap) changes its
    // value and re-runs this effect even though pathname is unchanged.
    // The effect body still reads window.location.search for the actual
    // URL state.
    void spString;

    // Build the canonical `?from=X&to=Y` from the current store and
    // write it if the address bar doesn't already match.
    const writeUrlFromStore = () => {
      const { from, to } = store.getState();
      const canonical = swapHref(from.stablecoin, to.stablecoin);
      const canonicalSearch = canonical.slice(canonical.indexOf("?"));
      if (window.location.search !== canonicalSearch) {
        const hash = window.location.hash;
        window.history.replaceState(null, "", `/swap${canonicalSearch}${hash}`);
      }
    };

    writeUrlFromStore();

    const onPopstate = () => {
      const sp = new URLSearchParams(window.location.search);
      const fromSlug = sp.get("from");
      const toSlug = sp.get("to");
      // Empty URL in history (e.g. back to a bare /swap visit) — store
      // wins; just canonicalize.
      if (!fromSlug && !toSlug) {
        writeUrlFromStore();
        return;
      }
      const pair = resolvePair(fromSlug, toSlug);
      const cur = store.getState();
      const matches =
        pair.from.currency === cur.from.currency &&
        pair.from.stablecoin === cur.from.stablecoin &&
        pair.to.currency === cur.to.currency &&
        pair.to.stablecoin === cur.to.stablecoin;
      if (!matches) cur.setSides(pair.from, pair.to);
      const canonical = swapHref(pair.from.stablecoin, pair.to.stablecoin);
      const canonicalSearch = canonical.slice(canonical.indexOf("?"));
      if (window.location.search !== canonicalSearch) {
        const hash = window.location.hash;
        window.history.replaceState(null, "", `/swap${canonicalSearch}${hash}`);
      }
    };

    window.addEventListener("popstate", onPopstate);
    return () => window.removeEventListener("popstate", onPopstate);
  }, [pathname, spString, store]);

  return null;
}
