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

type Store = {
  from: SideState;
  to: SideState;
  amount: string;
  activeSide: Side;
  setActiveSide: (side: Side) => void;
  setToken: (
    side: Side,
    currency: IsoCurrencyCode,
    stablecoin: string,
    cca2?: string,
  ) => void;
  setAmount: (amount: string) => void;
  swapSides: () => void;
};

export const useSwapStore = create<Store>((set) => ({
  from: { currency: "USD", stablecoin: "USDC", cca2: anchorFor("USD") },
  to: { currency: "EUR", stablecoin: "EURC", cca2: anchorFor("EUR") },
  amount: "",
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
