"use client";

import { useCallback, useSyncExternalStore } from "react";

// Reactive CSS media-query match via useSyncExternalStore (the tear-free
// pattern for subscribing to an external browser source). `serverDefault` is
// returned during SSR and the very first client render before hydration
// settles — pick the value matching the most common environment to minimize a
// post-hydration flash. Components rendered client-only (e.g. behind a
// `dynamic(..., { ssr: false })` boundary) read the real value from the first
// render, so the default never shows for them.
export function useMediaQuery(query: string, serverDefault = false): boolean {
  const subscribe = useCallback(
    (onChange: () => void) => {
      const mql = window.matchMedia(query);
      mql.addEventListener("change", onChange);
      return () => mql.removeEventListener("change", onChange);
    },
    [query],
  );
  return useSyncExternalStore(
    subscribe,
    () => window.matchMedia(query).matches,
    () => serverDefault,
  );
}
