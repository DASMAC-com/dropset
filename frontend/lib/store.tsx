"use client";

import { createContext, type ReactNode, useContext, useRef } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import { defaultAnchorCca2 } from "./countries";
import {
  currencyAnchor,
  DEFAULT_FROM,
  DEFAULT_TO,
  type IsoCurrencyCode,
  resolvePair,
  type SidePair,
} from "./currencies";

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

type Store = {
  from: SideState;
  to: SideState;
  amount: string;
  slippage: Slippage;
  activeSide: Side;
  setActiveSide: (side: Side) => void;
  // Assign `currency`/`stablecoin` to `side`. Refuses to write a sameToken
  // pair: if the requested token is already on the opposite side, atomically
  // flips the pair instead. Callers don't need to guard for this.
  setToken: (
    side: Side,
    currency: IsoCurrencyCode,
    stablecoin: string,
    cca2?: string,
  ) => void;
  setAmount: (amount: string) => void;
  setSlippage: (slippage: Slippage) => void;
  // Flip from/to. When `amount` is passed, it replaces the input amount in
  // the same `set` call so subscribers (notably the quote hook) never see a
  // transient state where sides have flipped but amount is still the pre-swap
  // value. Callers use this to promote the previous output amount into the
  // new input when toggling direction.
  swapSides: (amount?: string) => void;
  // Atomic from/to write used by URL reconciliation. Computes cca2 anchors
  // and writes both sides in a single `set`. activeSide is intentionally left
  // alone so the side asserted by a prior picker click survives this write.
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
    slippage: { mode: "auto" },
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
        if (side === "from" ? onTo : onFrom) {
          return { from: s.to, to: s.from, activeSide: side };
        }
        return {
          [side]: sideStateFor(currency, stablecoin, cca2),
          activeSide: side,
        };
      }),

    setAmount: (amount) => set({ amount }),

    setSlippage: (slippage) => set({ slippage }),

    swapSides: (amount) =>
      set((s) => ({
        from: s.to,
        to: s.from,
        activeSide: otherSide(s.activeSide),
        ...(amount !== undefined ? { amount } : {}),
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
