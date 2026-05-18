"use client";

import { usePathname, useSearchParams } from "next/navigation";
import { useEffect } from "react";
import { type IsoCurrencyCode, resolveTokenSlug } from "@/lib/currencies";
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
  // Each setToken call is gated on the resolved slug actually differing from
  // the current store, so the writer effect's re-fire doesn't clobber the
  // active side picked by upstream entry points (e.g. the currencies-page
  // pickers, which assert activeSide before navigating here).
  useEffect(() => {
    if (pathname !== "/swap") return;
    const f = resolveTokenSlug(searchParams.get("from"));
    const t = resolveTokenSlug(searchParams.get("to"));
    const slugConflict = !!(f && t && f.stablecoin === t.stablecoin);
    const apply = (
      side: "from" | "to",
      v: { currency: IsoCurrencyCode; stablecoin: string },
    ) => {
      const cur = useSwapStore.getState()[side];
      if (cur.currency === v.currency && cur.stablecoin === v.stablecoin)
        return;
      setToken(side, v.currency, v.stablecoin);
    };
    if (f) apply("from", f);
    if (t && !slugConflict) apply("to", t);

    const cur = useSwapStore.getState();
    if (cur.from.stablecoin !== cur.to.stablecoin) return;
    const fallback = (avoid: string) =>
      avoid === DEFAULT_TO_STABLECOIN
        ? {
            currency: DEFAULT_FROM_CURRENCY,
            stablecoin: DEFAULT_FROM_STABLECOIN,
          }
        : { currency: DEFAULT_TO_CURRENCY, stablecoin: DEFAULT_TO_STABLECOIN };
    if (f) apply("to", fallback(cur.from.stablecoin));
    else if (t) apply("from", fallback(cur.to.stablecoin));
  }, [pathname, searchParams, setToken]);

  // Always write current selection to URL while on /swap, including on first
  // arrival with defaults, after any picker change, and after a router-level
  // navigation that strips our params (e.g. clicking the favicon Link, which
  // points at bare /swap). Reading from searchParams in the body — rather than
  // window.location.search — ties this effect to Next.js' router updates so it
  // re-fires when nav changes the params underneath us. Params are rebuilt
  // from scratch so `from` always precedes `to` in the address bar, even if
  // the inbound URL had them reversed (URLSearchParams.set updates in place).
  useEffect(() => {
    if (pathname !== "/swap") return;
    const params = new URLSearchParams();
    params.set("from", fromSym);
    params.set("to", toSym);
    searchParams.forEach((value, key) => {
      if (key !== "from" && key !== "to") params.append(key, value);
    });
    const nextSearch = params.toString();
    if (searchParams.toString() === nextSearch) return;
    const next = `${window.location.pathname}?${nextSearch}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  }, [pathname, fromSym, toSym, searchParams]);

  return null;
}
