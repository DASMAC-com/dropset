import data from "./currencies.json";

export type IsoCurrencyCode = keyof typeof data;
export type Stablecoin = {
  symbol: string;
  name: string;
  mint: string;
  icon: string;
};
export type CurrencyEntry = {
  name: string;
  flag: string;
  anchorCca2: string;
  stablecoins: Stablecoin[];
};

export const CURRENCIES = data as Record<IsoCurrencyCode, CurrencyEntry>;

export const SUPPORTED: IsoCurrencyCode[] = Object.keys(
  CURRENCIES,
) as IsoCurrencyCode[];

const STABLE_BY_SYMBOL: Record<string, Stablecoin> = Object.fromEntries(
  SUPPORTED.flatMap((code) =>
    CURRENCIES[code].stablecoins.map((s) => [s.symbol, s]),
  ),
);

export const defaultStablecoin = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].stablecoins[0].symbol;

export const currencyName = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].name;

export const currencyFlag = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].flag;

export const currencyAnchor = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].anchorCca2;

export const tokenIconUrl = (symbol: string): string =>
  STABLE_BY_SYMBOL[symbol]?.icon ?? "";
