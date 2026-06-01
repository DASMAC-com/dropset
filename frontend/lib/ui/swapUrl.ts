"use client";

import type { Route } from "next";
import { useRouter } from "next/navigation";
import type { IsoCurrencyCode } from "@/lib/data/currencies";
import { useSwapStoreApi } from "@/lib/store";

// Canonical URL for a swap pair. Ordering is fixed (`from` then `to`) so the
// address bar reads the same regardless of which side mutated last.
export const swapHref = (from: string, to: string): string => {
  const params = new URLSearchParams({ from, to });
  return `/swap?${params.toString()}`;
};

// Navigate to /swap with a resolved pair. Same-route writes use
// history.replaceState to update the address bar without a router transition
// (so the URL canonicalization step from a picker click on /swap doesn't add
// a history entry). Cross-route writes use router.push so /currencies →
// /swap remains a real navigation that respects the back button.
//
// We read `window.location.pathname` at call time rather than via
// `usePathname()` because the React Compiler can memoize the returned
// closure with a stale render-time pathname — leading to replaceState on
// /currencies clicks (so the navigation never happens). `window.location`
// is always current at the moment the handler runs.
//
// The `as Route` cast on the dynamic href is required because typedRoutes
// only narrows string literals; a query-string built at runtime is opaque
// to the route type system.
export function useSwapNav(): (from: string, to: string) => void {
  const router = useRouter();
  return (from, to) => {
    const href = swapHref(from, to);
    if (window.location.pathname === "/swap") {
      window.history.replaceState(null, "", href);
    } else {
      router.push(href as Route);
    }
  };
}

// Load a full pair into the swap store and navigate to /swap. The store — not
// the URL — is the source of truth that survives cross-route navigation, so we
// write both sides via `setSides` BEFORE navigating (mirroring the per-token
// picker on /currencies); the URL is then just the canonical, shareable form.
// Writing only the URL would leave the persisted store on its defaults.
//
// `setSides` (the URL-reconciliation writer) intentionally preserves `amount`,
// but a deliberate pair pick follows `setToken`'s rule instead: a new from
// token clears the amount (the prior value was in the old from-token's units),
// while keeping the same from token leaves a typed amount alone.
export function useGoToSwapPair(): (
  from: { currency: IsoCurrencyCode; stablecoin: string },
  to: { currency: IsoCurrencyCode; stablecoin: string },
) => void {
  const store = useSwapStoreApi();
  const gotoSwap = useSwapNav();
  return (from, to) => {
    const state = store.getState();
    const fromChanged =
      state.from.currency !== from.currency ||
      state.from.stablecoin !== from.stablecoin;
    state.setSides(from, to);
    if (fromChanged) state.setAmount("");
    gotoSwap(from.stablecoin, to.stablecoin);
  };
}
