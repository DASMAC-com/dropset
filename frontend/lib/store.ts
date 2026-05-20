"use client";

import { create } from "zustand";
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

// Resolve the initial from/to pair from ?from=/?to= slugs on the URL, falling
// back to defaults whenever a slug is missing or unresolvable. Mirrors the
// conflict-resolution rules in UrlSync's reader effect (slugConflict, sameToken
// fallback) so a deep-link land matches what the reader would have produced
// post-hydration — without the visible "stitch" from defaults to URL values
// that an effect-driven hydration produced. Returns defaults on the server
// (`typeof window === "undefined"`), so SSR is deterministic and the per-process
// Zustand singleton is never mutated by a request's URL. On the client this
// runs once at module load, before any subscribing component renders.
const initialSides = (): { from: SideState; to: SideState } => {
  if (typeof window === "undefined") {
    return { from: defaultFrom(), to: defaultTo() };
  }
  const params = new URLSearchParams(window.location.search);
  const f = resolveTokenSlug(params.get("from"));
  const t = resolveTokenSlug(params.get("to"));
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

const initial = initialSides();

export const useSwapStore = create<Store>((set) => ({
  from: initial.from,
  to: initial.to,
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
      const onTo = currency === s.to.currency && stablecoin === s.to.stablecoin;
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
      const otherStable = side === "from" ? s.to.stablecoin : s.from.stablecoin;
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

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
