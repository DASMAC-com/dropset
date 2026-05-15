"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect } from "react";
import { resolveTokenSlug } from "@/lib/currencies";
import { useSwapStore } from "@/lib/store";

// Headless component that binds the swap store's from/to selection to the URL.
//   - When pathname is /swap, reads ?from / ?to and applies any resolvable
//     slugs, then writes the canonical ?from=<sym>&to=<sym> form back to the
//     address bar via history.replaceState. Re-runs whenever the path becomes
//     /swap so a nav from /currencies (or anywhere else) immediately gets the
//     slugs populated, even if no slugs were in the URL on entry.
export function UrlSync() {
  const searchParams = useSearchParams();
  const pathname = usePathname();
  const setToken = useSwapStore((s) => s.setToken);
  const fromSym = useSwapStore((s) => s.from.stablecoin);
  const toSym = useSwapStore((s) => s.to.stablecoin);

  // Hydrate from URL whenever we arrive on /swap (handles deep links and
  // back/forward nav). Reading searchParams in the deps subscribes us to
  // changes from Next.js' router (not our own replaceState writes).
  useEffect(() => {
    if (pathname !== "/swap") return;
    const f = resolveTokenSlug(searchParams.get("from"));
    if (f) setToken("from", f.currency, f.stablecoin);
    const t = resolveTokenSlug(searchParams.get("to"));
    if (t) setToken("to", t.currency, t.stablecoin);
  }, [pathname, searchParams, setToken]);

  // Always write current selection to URL while on /swap, including on first
  // arrival with defaults, after any picker change, and after a router-level
  // navigation that strips our params (e.g. clicking the favicon Link, which
  // points at bare /swap).
  useEffect(() => {
    if (pathname !== "/swap") return;
    const params = new URLSearchParams(window.location.search);
    if (params.get("from") === fromSym && params.get("to") === toSym) return;
    params.set("from", fromSym);
    params.set("to", toSym);
    const next = `${window.location.pathname}?${params.toString()}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  }, [pathname, fromSym, toSym, searchParams]);

  return null;
}
