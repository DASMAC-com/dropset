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
  const setSides = useSwapStore((s) => s.setSides);
  const fromSym = useSwapStore((s) => s.from.stablecoin);
  const toSym = useSwapStore((s) => s.to.stablecoin);

  // Hydrate from URL whenever we arrive on /swap (handles deep links and
  // back/forward nav). Computes the target from/to pair from the inbound
  // slugs, resolves any same-token outcome by replacing the side the user did
  // NOT specify with a non-conflicting default, then writes the whole pair in
  // a SINGLE `setState` call — sequential `setToken`s here would briefly leave
  // the store in a `from === to` state, which the GlobePanel observes as a
  // flash and is what produced the "wild flicker" report. activeSide is left
  // untouched so the side asserted by upstream entry points (e.g. the
  // currencies-page pickers) survives the writer-effect re-fire.
  useEffect(() => {
    if (pathname !== "/swap") return;
    const f = resolveTokenSlug(searchParams.get("from"));
    const t = resolveTokenSlug(searchParams.get("to"));
    if (!f && !t) return;
    const slugConflict = !!(f && t && f.stablecoin === t.stablecoin);

    const cur = useSwapStore.getState();
    let nextFrom: { currency: IsoCurrencyCode; stablecoin: string } = {
      currency: cur.from.currency,
      stablecoin: cur.from.stablecoin,
    };
    let nextTo: { currency: IsoCurrencyCode; stablecoin: string } = {
      currency: cur.to.currency,
      stablecoin: cur.to.stablecoin,
    };
    if (f) nextFrom = f;
    if (t && !slugConflict) nextTo = t;

    if (nextFrom.stablecoin === nextTo.stablecoin) {
      const fallback = (avoid: string) =>
        avoid === DEFAULT_TO_STABLECOIN
          ? {
              currency: DEFAULT_FROM_CURRENCY,
              stablecoin: DEFAULT_FROM_STABLECOIN,
            }
          : {
              currency: DEFAULT_TO_CURRENCY,
              stablecoin: DEFAULT_TO_STABLECOIN,
            };
      if (f) nextTo = fallback(nextFrom.stablecoin);
      else if (t) nextFrom = fallback(nextTo.stablecoin);
    }

    if (
      nextFrom.currency === cur.from.currency &&
      nextFrom.stablecoin === cur.from.stablecoin &&
      nextTo.currency === cur.to.currency &&
      nextTo.stablecoin === cur.to.stablecoin
    ) {
      return;
    }
    setSides(nextFrom, nextTo);
  }, [pathname, searchParams, setSides]);

  // Always write current selection to URL while on /swap, including on first
  // arrival with defaults, after any picker change, and after a router-level
  // navigation that strips our params (e.g. clicking the favicon Link, which
  // points at bare /swap). Reading from searchParams in the body — rather than
  // window.location.search — ties this effect to Next.js' router updates so it
  // re-fires when nav changes the params underneath us. Params are rebuilt
  // from scratch so `from` always precedes `to` in the address bar, even if
  // the inbound URL had them reversed (URLSearchParams.set updates in place).
  //
  // The from/to values are read via useSwapStore.getState() at fire-time
  // rather than from the fromSym/toSym render-phase bindings. The reader
  // effect above fires first in this same commit and may have just called
  // setSides() to reconcile the store with the URL — but the writer's
  // closure was captured BEFORE setSides ran, so using fromSym/toSym would
  // overwrite the URL with the values the reader just replaced. The next
  // reader pass would then re-apply the URL's (now-stale) values to the
  // store, and the two effects would ping-pong at ~40 Hz, tripping
  // Chromium's history-flooding throttle. Reading getState() makes the
  // writer always see the latest store snapshot — when the reader wins,
  // the bail-out below fires and the loop dies. fromSym/toSym stay in the
  // dep array so the effect still re-runs when the store changes.
  // biome-ignore lint/correctness/useExhaustiveDependencies: fromSym/toSym are subscription signals that re-fire this effect when the store mutates — intentionally in deps even though the body reads useSwapStore.getState()
  useEffect(() => {
    if (pathname !== "/swap") return;
    const { from, to } = useSwapStore.getState();
    const params = new URLSearchParams();
    params.set("from", from.stablecoin);
    params.set("to", to.stablecoin);
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
