"use client";

import { createContext, type ReactNode, useContext, useRef } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import { defaultAnchorCca2 } from "./data/countries";
import {
  currencyAnchor,
  DEFAULT_FROM,
  DEFAULT_TO,
  type IsoCurrencyCode,
  resolvePair,
  type SidePair,
} from "./data/currencies";
import { IS_LOCALNET } from "./env";

export type Side = "from" | "to";

export type SideState = {
  currency: IsoCurrencyCode;
  stablecoin: string;
  cca2: string;
};

const otherSide = (s: Side): Side => (s === "from" ? "to" : "from");

const anchorFor = (currency: IsoCurrencyCode): string =>
  currencyAnchor(currency) || defaultAnchorCca2(currency);

const sideStateFor = (
  currency: IsoCurrencyCode,
  stablecoin: string,
  cca2?: string,
): SideState => ({
  currency,
  stablecoin,
  cca2: cca2 ?? anchorFor(currency),
});

const sidesFromPair = (pair: SidePair): { from: SideState; to: SideState } => ({
  from: sideStateFor(pair.from.currency, pair.from.stablecoin),
  to: sideStateFor(pair.to.currency, pair.to.stablecoin),
});

export const DEFAULT_FROM_CURRENCY: IsoCurrencyCode = DEFAULT_FROM.currency;
export const DEFAULT_FROM_STABLECOIN: string = DEFAULT_FROM.stablecoin;
export const DEFAULT_TO_CURRENCY: IsoCurrencyCode = DEFAULT_TO.currency;
export const DEFAULT_TO_STABLECOIN: string = DEFAULT_TO.stablecoin;

export type Slippage = { mode: "auto" } | { mode: "fixed"; percent: number };

// How the swap is routed. `best` uses the DFlow aggregator (best price across
// venues); `eclob` routes directly through the Dropset SDK against our own
// market. Localnet forces `eclob` — the aggregator only knows mainnet
// liquidity and can't see the local market (see setRouteMode).
export type RouteMode = "best" | "eclob";

type Store = {
  from: SideState;
  to: SideState;
  amount: string;
  // Latest formatted to-side decimal string from the live quote (in
  // to-decimals' units). Kept here — not in component state — so that
  // mutation actions can read it without the caller having to know the
  // quote. Empty string when no live quote is available.
  //
  // Crucially this persists across cross-route navigation: a picker
  // click on /currencies (where the quote hook isn't mounted) can still
  // read the value the quote produced just before the user navigated
  // away, and use it as the promoted from-amount on a direction flip.
  lastFormattedOutAmount: string;
  slippage: Slippage;
  routeMode: RouteMode;
  activeSide: Side;
  setActiveSide: (side: Side) => void;
  // Assign `currency`/`stablecoin` to `side`. Four branches:
  //   * Token already on requested side → no-op (just refresh activeSide).
  //   * Token already on the *opposite* side → atomically flip the pair
  //     and promote `lastFormattedOutAmount` into `amount` so the
  //     previous to-side value becomes the new from-side input. If no
  //     live quote, the existing amount is left alone.
  //   * Otherwise, picking a new **from** token → set the side and clear
  //     `amount` (the prior value was in the old from-token's units and
  //     would be misinterpreted under the new from-decimals).
  //   * Otherwise, picking a new **to** token → set the side and leave
  //     `amount` alone (the from-side hasn't changed, so the typed
  //     amount is still meaningful).
  setToken: (
    side: Side,
    currency: IsoCurrencyCode,
    stablecoin: string,
    cca2?: string,
  ) => void;
  setAmount: (amount: string) => void;
  setSlippage: (slippage: Slippage) => void;
  // Set the route mode. Clamped to `eclob` on localnet regardless of the
  // requested value, so the UI toggle can't route a local swap through the
  // aggregator (which would quote against mainnet, not the local market).
  setRouteMode: (mode: RouteMode) => void;
  setLastFormattedOutAmount: (formatted: string) => void;
  // Flip from/to. Reads `lastFormattedOutAmount` from the store and
  // promotes it into `amount` in the same `set` call so subscribers
  // (notably the quote hook) never see a transient state where sides
  // have flipped but amount is still the pre-swap value. If no live
  // quote is available, leaves the existing amount alone.
  swapSides: () => void;
  // Atomic from/to write used by URL reconciliation. Computes cca2 anchors
  // and writes both sides in a single `set`. activeSide and amount are
  // intentionally left alone so the side asserted by a prior picker click
  // (and the user's typed amount) survive this write.
  setSides: (
    from: { currency: IsoCurrencyCode; stablecoin: string },
    to: { currency: IsoCurrencyCode; stablecoin: string },
  ) => void;
};

// Factory for a fresh Zustand store instance. An optional initial pair seeds
// `from`/`to` at construction time so the store is born with the URL-derived
// state (no setState during render, no initializer race).
export const createSwapStore = (initial?: {
  from: SideState;
  to: SideState;
}): StoreApi<Store> =>
  createStore<Store>((set) => ({
    from:
      initial?.from ??
      sideStateFor(DEFAULT_FROM.currency, DEFAULT_FROM.stablecoin),
    to: initial?.to ?? sideStateFor(DEFAULT_TO.currency, DEFAULT_TO.stablecoin),
    amount: "",
    lastFormattedOutAmount: "",
    slippage: { mode: "auto" },
    // Localnet has no aggregator route, so it opens (and stays) on eCLOB-only.
    routeMode: IS_LOCALNET ? "eclob" : "best",
    activeSide: "from",

    setActiveSide: (side) => set({ activeSide: side }),

    setToken: (side, currency, stablecoin, cca2) =>
      set((s) => {
        const onFrom =
          currency === s.from.currency && stablecoin === s.from.stablecoin;
        const onTo =
          currency === s.to.currency && stablecoin === s.to.stablecoin;
        // No-op when the token is already on the requested side; just refresh
        // activeSide so the swap page highlights the matching row.
        if (side === "from" ? onFrom : onTo) return { activeSide: side };
        // Token is on the opposite side — flip the pair atomically so we
        // never observe a transient sameToken between two sequential `set`s.
        // The previous to-side displayed amount (lastFormattedOutAmount) is
        // already in to-decimals' units, which is exactly the new from-side's
        // unit basis after the flip — so promote it into `amount`.
        if (side === "from" ? onTo : onFrom) {
          return {
            from: s.to,
            to: s.from,
            activeSide: side,
            ...(s.lastFormattedOutAmount
              ? { amount: s.lastFormattedOutAmount }
              : {}),
            // Reset the cached out-amount; it was for the pre-flip pair
            // and now it's been consumed (or there's nothing to consume).
            lastFormattedOutAmount: "",
          };
        }
        // New token replaces this side. When that side is `from`, the
        // existing `amount` was in the OLD from-token's units and is
        // now meaningless — clear it. When it's `to`, the from-side
        // hasn't changed and the user's typed amount is still valid;
        // keep it so swapping the destination doesn't make them retype.
        return {
          [side]: sideStateFor(currency, stablecoin, cca2),
          activeSide: side,
          ...(side === "from" ? { amount: "" } : {}),
          lastFormattedOutAmount: "",
        };
      }),

    setAmount: (amount) => set({ amount }),

    setSlippage: (slippage) => set({ slippage }),

    setRouteMode: (mode) => set({ routeMode: IS_LOCALNET ? "eclob" : mode }),

    setLastFormattedOutAmount: (formatted) =>
      set({ lastFormattedOutAmount: formatted }),

    swapSides: () =>
      set((s) => ({
        from: s.to,
        to: s.from,
        activeSide: otherSide(s.activeSide),
        ...(s.lastFormattedOutAmount
          ? { amount: s.lastFormattedOutAmount }
          : {}),
        lastFormattedOutAmount: "",
      })),

    setSides: (from, to) =>
      set({
        from: sideStateFor(from.currency, from.stablecoin),
        to: sideStateFor(to.currency, to.stablecoin),
      }),
  }));

const SwapStoreContext = createContext<StoreApi<Store> | null>(null);

// Per-render-tree store holder. The store is created once via `useRef` and
// kept alive for the entire app session, so /currencies and /swap share the
// same instance across client-side navigation.
//
// On its first client mount the factory reads ?from / ?to off
// window.location.search and seeds the store with the resolved pair — so a
// deep-link load paints the URL-derived state on the very first frame. SSR
// runs the factory without window and falls through to defaults; the
// resulting hydration mismatch on URL-deep-linked pages is recovered
// silently by React 19 in production.
export function SwapStoreProvider({ children }: { children: ReactNode }) {
  const ref = useRef<StoreApi<Store> | null>(null);
  if (ref.current === null) {
    let initial: { from: SideState; to: SideState } | undefined;
    if (typeof window !== "undefined") {
      const params = new URLSearchParams(window.location.search);
      const f = params.get("from");
      const t = params.get("to");
      if (f || t) initial = sidesFromPair(resolvePair(f, t));
    }
    ref.current = createSwapStore(initial);
  }
  return (
    <SwapStoreContext.Provider value={ref.current}>
      {children}
    </SwapStoreContext.Provider>
  );
}

const useStoreApi = (): StoreApi<Store> => {
  const store = useContext(SwapStoreContext);
  if (store === null) {
    throw new Error("Swap store accessed outside of <SwapStoreProvider>");
  }
  return store;
};

export function useSwapStore<T>(selector: (state: Store) => T): T {
  return useStore(useStoreApi(), selector);
}

// Raw store handle for code that needs imperative `.getState`/`.setState`
// access outside a selector (e.g. UrlSync's reconciliation effect).
export const useSwapStoreApi = useStoreApi;

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
