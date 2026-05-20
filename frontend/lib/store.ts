"use client";

import {
  createContext,
  type ReactNode,
  useContext,
  useRef,
  useState,
} from "react";
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

// Resolve raw ?from=/?to= slug strings into a canonical (from, to) pair using
// the same conflict-resolution rules UrlSync's reader applies: slugConflict
// (both resolve to the same stable), sameToken fallback (the unspecified side
// is replaced with a non-conflicting default), and absent/unresolvable slugs
// fall back to USDC/EURC. Pure — no React, no DOM — so it runs identically on
// the server (where the page resolves searchParams) and on the client (where
// SwapStateInitializer applies the result). Keeping this here, alongside the
// store schema, means the conflict semantics are defined once and referenced
// by both the seeder and the live URL-sync reader.
export const resolveInitialSides = (
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

// Factory for a fresh Zustand store instance, used by SwapStoreProvider. One
// store per render tree, so each request on the server gets its own (no
// cross-request bleed of the module-level singleton) and the client gets one
// for the lifetime of the page. Callers may pass an initial from/to pair to
// seed the store before its first read (e.g. URL-derived state from the page
// server component); otherwise the store comes up on defaults.
export const createSwapStore = (
  initial?: { from: SideState; to: SideState },
): StoreApi<Store> =>
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
        const otherCurrency =
          side === "from" ? s.to.currency : s.from.currency;
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

// Holds a per-render-tree Zustand store instance via useRef. Mounting this in
// the root layout means /swap and /currencies share the same store across
// client-side navigation (token picks on /currencies are visible on /swap),
// while each server request gets its own instance — no module-level singleton
// to bleed state across users.
export function SwapStoreProvider({ children }: { children: ReactNode }) {
  const ref = useRef<StoreApi<Store> | null>(null);
  if (ref.current === null) ref.current = createSwapStore();
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

// Raw store handle for code that needs imperative .getState/.setState access
// outside a selector (e.g. UrlSync's effects, which need to read the current
// pair at fire-time rather than via render-phase closure bindings).
export const useSwapStoreApi = useStoreApi;

// Apply a URL-derived initial pair to the store ONCE per mount of this hook,
// via useState's lazy initializer. Running through a lazy init guarantees the
// store mutation happens synchronously during render — before any subscribing
// child reads the store — on both the SSR pass and client hydration. Repeated
// renders are no-ops; navigation that remounts the page re-seeds with the new
// URL pair.
export function useSeedSwapStore(initial: {
  from: SideState;
  to: SideState;
}): void {
  const api = useStoreApi();
  useState(() => {
    api.setState({ from: initial.from, to: initial.to });
    return null;
  });
}

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
