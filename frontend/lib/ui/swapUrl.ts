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

// Canonical URL for the Vaults tab filtered to a pair (and optionally a single
// leader). `base`/`quote` are stablecoin symbols; `leader` is a base58 pubkey
// or prefix. The page reads these to pin its view, and they double as a
// shareable deep link to a specific vault. Symmetric to `swapHref`.
export const vaultsHref = (
  base: string,
  quote: string,
  leader?: string,
): string => {
  const params = new URLSearchParams({ base, quote });
  if (leader) params.set("leader", leader);
  return `/vaults?${params.toString()}`;
};

// Set/delete query params on the current URL via replaceState (no router
// transition), preserving the other params + hash. A falsy value deletes the
// key. Shared by the vaults pin filter and the currencies search-URL sync.
export const replaceUrlParams = (
  updates: Record<string, string | null>,
): void => {
  const params = new URLSearchParams(window.location.search);
  for (const [key, value] of Object.entries(updates)) {
    if (value) params.set(key, value);
    else params.delete(key);
  }
  const search = params.toString();
  window.history.replaceState(
    null,
    "",
    `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`,
  );
};

// Navigate to the Vaults tab filtered to a pair. Vaults state lives entirely in
// the URL (there's no persisted store like swap's), so a plain push is the
// whole handoff — and the resulting URL is shareable as-is.
export function useGoToVaultsForPair(): (
  base: string,
  quote: string,
  leader?: string,
) => void {
  const router = useRouter();
  return (base, quote, leader) =>
    router.push(vaultsHref(base, quote, leader) as Route);
}

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
    // Drop the previous pair's cached out-amount so a later swapSides can't
    // promote a stale value before the first fresh quote lands.
    state.setLastFormattedOutAmount("");
    gotoSwap(from.stablecoin, to.stablecoin);
  };
}
