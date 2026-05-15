"use client";

import { create } from "zustand";
import { defaultAnchorCca2 } from "./countries";
import { currencyAnchor, type IsoCurrencyCode } from "./currencies";

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
  swapSides: () => void;
};

export const DEFAULT_FROM_CURRENCY: IsoCurrencyCode = "USD";
export const DEFAULT_FROM_STABLECOIN = "USDC";
export const DEFAULT_TO_CURRENCY: IsoCurrencyCode = "EUR";
export const DEFAULT_TO_STABLECOIN = "EURC";

export const useSwapStore = create<Store>((set) => ({
  from: {
    currency: DEFAULT_FROM_CURRENCY,
    stablecoin: DEFAULT_FROM_STABLECOIN,
    cca2: anchorFor(DEFAULT_FROM_CURRENCY),
  },
  to: {
    currency: DEFAULT_TO_CURRENCY,
    stablecoin: DEFAULT_TO_STABLECOIN,
    cca2: anchorFor(DEFAULT_TO_CURRENCY),
  },
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

  swapSides: () =>
    set((s) => ({
      from: s.to,
      to: s.from,
      activeSide: otherSide(s.activeSide),
    })),
}));

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
