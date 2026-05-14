import data from "./currencies.json";

export type IsoCurrencyCode = keyof typeof data;
export type Stablecoin = {
  symbol: string;
  name: string;
  mint: string;
  decimals: number;
  icon: string;
};
export type CurrencyEntry = {
  name: string;
  flag?: string;
  anchorCca2?: string;
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

const deriveFlag = (code: string): string =>
  String.fromCodePoint(
    ...[...code.slice(0, 2)].map((c) => 0x1f1e6 + c.charCodeAt(0) - 65),
  );

export const currencyFlag = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].flag ?? deriveFlag(code);

export const currencyAnchor = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].anchorCca2 ?? code.slice(0, 2);

export const tokenIconUrl = (symbol: string): string =>
  STABLE_BY_SYMBOL[symbol]?.icon ?? "";

export const stablecoinDecimals = (symbol: string): number =>
  STABLE_BY_SYMBOL[symbol]?.decimals ?? 0;

export const stablecoinMint = (symbol: string): string =>
  STABLE_BY_SYMBOL[symbol]?.mint ?? "";
