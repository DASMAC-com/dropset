"use client";

import { create } from "zustand";
import { type CountryPin, defaultAnchorCca2 } from "./countries";
import { CURRENCIES, currencyAnchor, type IsoCurrencyCode } from "./currencies";

export type Side = "from" | "to";

export type SideState = {
  currency: IsoCurrencyCode;
  stablecoin: string;
  cca2: string;
};

const otherSide = (s: Side): Side => (s === "from" ? "to" : "from");

const wouldCollide = (
  currency: IsoCurrencyCode,
  stablecoin: string,
  other: SideState,
): boolean => currency === other.currency && stablecoin === other.stablecoin;

const pickNonCollidingStable = (
  currency: IsoCurrencyCode,
  other: SideState,
): string => {
  const stables = CURRENCIES[currency].stablecoins;
  const choice = stables.find((s) => !wouldCollide(currency, s.symbol, other));
  return (choice ?? stables[0]).symbol;
};

const shouldOpenPicker = (
  currency: IsoCurrencyCode,
  other: SideState,
): boolean => {
  const usable = CURRENCIES[currency].stablecoins.filter(
    (s) => !wouldCollide(currency, s.symbol, other),
  );
  return usable.length > 1;
};

const anchorFor = (currency: IsoCurrencyCode): string =>
  currencyAnchor(currency) || defaultAnchorCca2(currency);

type Store = {
  from: SideState;
  to: SideState;
  activeSide: Side;
  openStablecoinPickerFor: Side | null;
  setActiveSide: (side: Side) => void;
  setToken: (side: Side, currency: IsoCurrencyCode, stablecoin: string) => void;
  setPinClicked: (pin: CountryPin) => void;
  swapSides: () => void;
  clearStablecoinPickerSignal: () => void;
};

export const useSwapStore = create<Store>((set) => ({
  from: { currency: "USD", stablecoin: "USDC", cca2: anchorFor("USD") },
  to: { currency: "EUR", stablecoin: "EURC", cca2: anchorFor("EUR") },
  activeSide: "from",
  openStablecoinPickerFor: null,

  setActiveSide: (side) => set({ activeSide: side }),

  setToken: (side, currency, stablecoin) =>
    set({
      [side]: { currency, stablecoin, cca2: anchorFor(currency) },
      activeSide: side,
      openStablecoinPickerFor: null,
    }),

  setPinClicked: (pin) =>
    set((s) => {
      const side = s.activeSide;
      const other = s[otherSide(side)];
      const stable = pickNonCollidingStable(pin.currency, other);
      return {
        [side]: { currency: pin.currency, stablecoin: stable, cca2: pin.cca2 },
        openStablecoinPickerFor: shouldOpenPicker(pin.currency, other)
          ? side
          : null,
      };
    }),

  swapSides: () =>
    set((s) => ({
      from: s.to,
      to: s.from,
      activeSide: otherSide(s.activeSide),
      openStablecoinPickerFor: null,
    })),

  clearStablecoinPickerSignal: () => set({ openStablecoinPickerFor: null }),
}));

export const useSameToken = (): boolean =>
  useSwapStore(
    (s) =>
      s.from.currency === s.to.currency &&
      s.from.stablecoin === s.to.stablecoin,
  );
