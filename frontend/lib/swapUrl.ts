"use client";

import type { Route } from "next";
import { useRouter } from "next/navigation";

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
