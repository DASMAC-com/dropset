"use client";

import { useSearchParams } from "next/navigation";
import { useEffect, useRef } from "react";
import { resolveTokenSlug } from "@/lib/currencies";
import {
  DEFAULT_FROM_STABLECOIN,
  DEFAULT_TO_STABLECOIN,
  useSwapStore,
} from "@/lib/store";

// Headless component that binds the swap store's from/to selection to the URL:
//   - On mount, reads `?from=` and `?to=` and applies any resolvable slugs.
//   - After hydration, watches the selected stablecoin symbols and writes the
//     canonical `?from=<sym>&to=<sym>` form back to the address bar via
//     history.replaceState — but leaves a pristine `/` visit alone.
export function UrlSync() {
  const searchParams = useSearchParams();
  const setToken = useSwapStore((s) => s.setToken);
  const fromSym = useSwapStore((s) => s.from.stablecoin);
  const toSym = useSwapStore((s) => s.to.stablecoin);
  const hydratedRef = useRef(false);

  // biome-ignore lint/correctness/useExhaustiveDependencies: hydration is one-shot on mount; deps would re-fire on URL changes we just wrote ourselves
  useEffect(() => {
    if (hydratedRef.current) return;
    hydratedRef.current = true;
    const f = resolveTokenSlug(searchParams.get("from"));
    if (f) setToken("from", f.currency, f.stablecoin);
    const t = resolveTokenSlug(searchParams.get("to"));
    if (t) setToken("to", t.currency, t.stablecoin);
  }, []);

  useEffect(() => {
    if (!hydratedRef.current) return;
    const params = new URLSearchParams(window.location.search);
    const urlHasSlugs = params.has("from") || params.has("to");
    const isDefault =
      fromSym === DEFAULT_FROM_STABLECOIN && toSym === DEFAULT_TO_STABLECOIN;
    if (isDefault && !urlHasSlugs) return;
    if (params.get("from") === fromSym && params.get("to") === toSym) return;
    params.set("from", fromSym);
    params.set("to", toSym);
    const next = `${window.location.pathname}?${params.toString()}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  }, [fromSym, toSym]);

  return null;
}
