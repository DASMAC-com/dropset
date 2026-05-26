import type { IsoCurrencyCode } from "../data/currencies";
import type { Side } from "../store";

// Tiny predicates shared by every picker surface (TokenPicker dropdown,
// CurrenciesView row, GlobePanel country dialog). Each side's picker
// disables the button when the token is already on that side, and offers
// a "flip direction" affordance when it's on the opposite side — the
// rules below codify both.

export type Selection = {
  fromCurrency: IsoCurrencyCode;
  fromStablecoin: string;
  toCurrency: IsoCurrencyCode;
  toStablecoin: string;
};

export const isOnFrom = (
  sel: Selection,
  currency: IsoCurrencyCode,
  symbol: string,
): boolean => currency === sel.fromCurrency && symbol === sel.fromStablecoin;

export const isOnTo = (
  sel: Selection,
  currency: IsoCurrencyCode,
  symbol: string,
): boolean => currency === sel.toCurrency && symbol === sel.toStablecoin;

// True when the requested (side, token) is already on the *other* side —
// i.e. picking it would only be valid as a direction flip.
export const isOnOpposite = (
  sel: Selection,
  side: Side,
  currency: IsoCurrencyCode,
  symbol: string,
): boolean =>
  side === "from"
    ? isOnTo(sel, currency, symbol)
    : isOnFrom(sel, currency, symbol);

// True when the requested (side, token) is already on the requested side
// — the picker should disable the button to prevent a no-op click.
export const isOnSide = (
  sel: Selection,
  side: Side,
  currency: IsoCurrencyCode,
  symbol: string,
): boolean =>
  side === "from"
    ? isOnFrom(sel, currency, symbol)
    : isOnTo(sel, currency, symbol);
