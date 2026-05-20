"use client";

import { createContext, type ReactNode, useContext, useRef } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import { defaultAnchorCca2 } from "./countries";
import {
  currencyAnchor,
  type IsoCurrencyCode,
  resolveTokenSlug,
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

export type Slippage = { mode: "auto" } | { mode: "fixed"; percent: number };

type Store = {
  from: SideState;
  to: SideState;
  amount: string;
  slippage: Slippage;
  activeSide: Side;
  setActiveSide: (side: Side) => void;
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
  // Atomic combination of swapSides / setToken / setActiveSide. Reads the
  // current state, decides which mutation matches the click intent, and writes
  // the result in a single `set` call so subscribers never observe a transient
  // `from === to` state. Refuses to produce a sameToken result (callers should
  // disable the corresponding button, but this is also a defensive guard).
  pickSide: (side: Side, currency: IsoCurrencyCode, stablecoin: string) => void;
  // Atomic from/to write used by URL hydration. Computes cca2 anchors and
  // writes both sides in a single `set` so the GlobePanel never observes a
  // transient sameToken state between two sequential `setToken` calls.
  // activeSide is intentionally left alone so it survives the writer-effect
  // re-fire that follows replaceState.
  setSides: (
    from: { currency: IsoCurrencyCode; stablecoin: string },
    to: { currency: IsoCurrencyCode; stablecoin: string },
  ) => void;
};

export const DEFAULT_FROM_CURRENCY: IsoCurrencyCode = "USD";
export const DEFAULT_FROM_STABLECOIN = "USDC";
export const DEFAULT_TO_CURRENCY: IsoCurrencyCode = "EUR";
export const DEFAULT_TO_STABLECOIN = "EURC";

const sideFor = (currency: IsoCurrencyCode, stablecoin: string): SideState => ({
  currency,
  stablecoin,
  cca2: anchorFor(currency),
});
const defaultFrom = (): SideState =>
  sideFor(DEFAULT_FROM_CURRENCY, DEFAULT_FROM_STABLECOIN);
const defaultTo = (): SideState =>
  sideFor(DEFAULT_TO_CURRENCY, DEFAULT_TO_STABLECOIN);

// Pure helper that turns raw ?from / ?to slug strings into a canonical pair,
// applying the same conflict / sameToken rules UrlSync's reader uses: both
// missing → defaults; both present and identical → resolve via fallback so
// the two sides differ; only one present → keep the other as a non-
// conflicting default. No React, no DOM — runs identically wherever called.
const resolveInitialSides = (
  fromSlug: string | null | undefined,
  toSlug: string | null | undefined,
): { from: SideState; to: SideState } => {
  const f = resolveTokenSlug(fromSlug);
  const t = resolveTokenSlug(toSlug);
  if (!f && !t) return { from: defaultFrom(), to: defaultTo() };
  const slugConflict = !!(f && t && f.stablecoin === t.stablecoin);
  let from = f ? sideFor(f.currency, f.stablecoin) : defaultFrom();
  let to = t && !slugConflict ? sideFor(t.currency, t.stablecoin) : defaultTo();
  if (from.stablecoin === to.stablecoin) {
    const fallback = (avoid: string): SideState =>
      avoid === DEFAULT_TO_STABLECOIN ? defaultFrom() : defaultTo();
    if (f) to = fallback(from.stablecoin);
    else if (t) from = fallback(to.stablecoin);
  }
  return { from, to };
};

// Factory for a fresh Zustand store instance, used by SwapStoreProvider. An
// optional initial pair seeds `from`/`to` at construction time so the store
// is born with the URL-derived state — no setState during render, no
// initializer race. Actions are otherwise identical to the previous
// module-level singleton.
export const createSwapStore = (initial?: {
  from: SideState;
  to: SideState;
}): StoreApi<Store> =>
  createStore<Store>((set) => ({
    from: initial?.from ?? defaultFrom(),
    to: initial?.to ?? defaultTo(),
    amount: "",
    slippage: { mode: "auto" },
    activeSide: "from",

    setActiveSide: (side) => set({ activeSide: side }),

    setToken: (side, currency, stablecoin, cca2) =>
      set({
        [side]: {
          currency,
          stablecoin,
          cca2: cca2 ?? anchorFor(currency),
        },
        activeSide: side,
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
        from: {
          currency: from.currency,
          stablecoin: from.stablecoin,
          cca2: anchorFor(from.currency),
        },
        to: {
          currency: to.currency,
          stablecoin: to.stablecoin,
          cca2: anchorFor(to.currency),
        },
      }),

    pickSide: (side, currency, stablecoin) =>
      set((s) => {
        const onFrom =
          currency === s.from.currency && stablecoin === s.from.stablecoin;
        const onTo =
          currency === s.to.currency && stablecoin === s.to.stablecoin;
        // Token already on the requested side — only refresh activeSide so the
        // swap page highlights the matching row, but leave the pair untouched.
        if (side === "from" ? onFrom : onTo) return { activeSide: side };
        // Token on the opposite side — flip the pair atomically so we never
        // observe a transient sameToken between two sequential `set`s.
        if (side === "from" ? onTo : onFrom) {
          return { from: s.to, to: s.from, activeSide: side };
        }
        // Refuse to write a sameToken pair. Only reachable if a caller bypasses
        // the disabled-button state (e.g. a stale event fires during nav).
        const otherStable =
          side === "from" ? s.to.stablecoin : s.from.stablecoin;
        const otherCurrency = side === "from" ? s.to.currency : s.from.currency;
        if (stablecoin === otherStable && currency === otherCurrency) {
          return { activeSide: side };
        }
        return {
          [side]: { currency, stablecoin, cca2: anchorFor(currency) },
          activeSide: side,
        };
      }),
  }));

const SwapStoreContext = createContext<StoreApi<Store> | null>(null);

// Per-render-tree store holder. The store is created once via `useRef` and
// kept alive for the entire app session, so /currencies and /swap share the
// same instance across client-side navigation (a pickSide on /currencies is
// visible on /swap, no clobbering).
//
// On its first client mount the factory reads ?from / ?to off
// window.location.search and seeds the store with the resolved pair — so a
// deep-link load paints the URL-derived state on the very first frame,
// without an effect-driven snap. Subsequent renders skip the factory
// (`ref.current !== null`) so navigation doesn't re-init. SSR runs the
// factory with `window === undefined`, falling through to defaults — the
// resulting hydration mismatch on URL-deep-linked pages is recovered
// silently by React 19 in production; dev logs a warning. We deliberately
// avoid a setState-driven initializer here because mutating the store
// during render notifies subscribers mid-render (which on /currencies →
// /swap transitions reaches still-rendering SwapPickerCells and throws
// "Cannot update a component while rendering").
export function SwapStoreProvider({ children }: { children: ReactNode }) {
  const ref = useRef<StoreApi<Store> | null>(null);
  if (ref.current === null) {
    let initial: { from: SideState; to: SideState } | undefined;
    if (typeof window !== "undefined") {
      const params = new URLSearchParams(window.location.search);
      const f = params.get("from");
      const t = params.get("to");
      if (f || t) initial = resolveInitialSides(f, t);
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

// Selector hook — drop-in replacement for the prior `useSwapStore(selector)`
// API; subscribes the component to the slice the selector returns.
export function useSwapStore<T>(selector: (state: Store) => T): T {
  return useStore(useStoreApi(), selector);
}

// Raw store handle for code that needs imperative `.getState`/`.setState`
// access outside a selector (e.g. UrlSync's effects, which read the current
// pair at fire-time rather than via render-phase closure bindings).
export const useSwapStoreApi = useStoreApi;

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
