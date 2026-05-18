"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect } from "react";
import { resolveTokenSlug } from "@/lib/currencies";
import {
  DEFAULT_FROM_CURRENCY,
  DEFAULT_FROM_STABLECOIN,
  DEFAULT_TO_CURRENCY,
  DEFAULT_TO_STABLECOIN,
  useSwapStore,
} from "@/lib/store";

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
  // changes from Next.js' router (not our own replaceState writes). After
  // applying slugs, resolve any same-token state by replacing the side the
  // user did NOT specify with a non-conflicting default — this handles both
  // `?from=X&to=X` and the subtler `?to=X` where the default `from` is also X.
  useEffect(() => {
    if (pathname !== "/swap") return;
    const f = resolveTokenSlug(searchParams.get("from"));
    const t = resolveTokenSlug(searchParams.get("to"));
    const slugConflict = !!(f && t && f.stablecoin === t.stablecoin);
    if (f) setToken("from", f.currency, f.stablecoin);
    if (t && !slugConflict) setToken("to", t.currency, t.stablecoin);

    const cur = useSwapStore.getState();
    if (cur.from.stablecoin !== cur.to.stablecoin) return;
    const fallback = (avoid: string) =>
      avoid === DEFAULT_TO_STABLECOIN
        ? {
            currency: DEFAULT_FROM_CURRENCY,
            stablecoin: DEFAULT_FROM_STABLECOIN,
          }
        : { currency: DEFAULT_TO_CURRENCY, stablecoin: DEFAULT_TO_STABLECOIN };
    if (f) {
      const fb = fallback(cur.from.stablecoin);
      setToken("to", fb.currency, fb.stablecoin);
    } else if (t) {
      const fb = fallback(cur.to.stablecoin);
      setToken("from", fb.currency, fb.stablecoin);
    }
  }, [pathname, searchParams, setToken]);

  // Always write current selection to URL while on /swap, including on first
  // arrival with defaults, after any picker change, and after a router-level
  // navigation that strips our params (e.g. clicking the favicon Link, which
  // points at bare /swap). Reading from searchParams in the body — rather than
  // window.location.search — ties this effect to Next.js' router updates so it
  // re-fires when nav changes the params underneath us.
  useEffect(() => {
    if (pathname !== "/swap") return;
    if (
      searchParams.get("from") === fromSym &&
      searchParams.get("to") === toSym
    )
      return;
    const params = new URLSearchParams(searchParams.toString());
    params.set("from", fromSym);
    params.set("to", toSym);
    const next = `${window.location.pathname}?${params.toString()}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  }, [pathname, fromSym, toSym, searchParams]);

  return null;
}
