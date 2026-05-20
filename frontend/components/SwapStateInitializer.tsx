"use client";

import { useState } from "react";
import { resolveInitialSides, useSwapStoreApi } from "@/lib/store";

// Seeds the per-tree swap store from the URL's ?from / ?to slugs once per
// mount. Designed to be rendered by the /swap server component (which has
// access to searchParams) as the first child inside the layout's
// SwapStoreProvider; subsequent siblings (SwapPanel, UrlSync, GlobePanel)
// then read the URL-derived pair on their first render — eliminating the
// brief flash of defaults that an effect-driven hydration produced.
//
// useState's lazy initializer is the run-once-per-mount escape hatch we want:
// it executes synchronously during render, on both the SSR pass and client
// hydration, so server-rendered HTML and the hydrated tree reach the same
// pair from identical inputs (no mismatch). Re-renders without remount are
// no-ops; client-side navigation that remounts /swap re-seeds with whatever
// slugs the new entry carries.
export function SwapStateInitializer({
  fromSlug,
  toSlug,
}: {
  fromSlug?: string;
  toSlug?: string;
}) {
  const api = useSwapStoreApi();
  useState(() => {
    const { from, to } = resolveInitialSides(fromSlug, toSlug);
    api.setState({ from, to });
    return null;
  });
  return null;
}
