import countries from "world-countries";
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
  flag?: string;
  anchorCca2?: string;
  stablecoins: Stablecoin[];
};

export const CURRENCIES = data as Record<IsoCurrencyCode, CurrencyEntry>;

const NAME_BY_CODE: Record<string, string> = {};
for (const c of countries) {
  for (const [code, info] of Object.entries(c.currencies ?? {})) {
    if (!NAME_BY_CODE[code]) NAME_BY_CODE[code] = info.name;
  }
}

export const SUPPORTED: IsoCurrencyCode[] = Object.keys(
  CURRENCIES,
) as IsoCurrencyCode[];

const STABLE_BY_SYMBOL: Record<string, Stablecoin> = Object.fromEntries(
  SUPPORTED.flatMap((code) =>
    CURRENCIES[code].stablecoins.map((s) => [s.symbol, s]),
  ),
);

// Case-insensitive lookup that yields the canonical-cased symbol (e.g. the
// JSON's `tGBP` rather than `TGBP`). Used by URL slug resolution.
const SYMBOL_BY_UPPER: Record<string, string> = Object.fromEntries(
  SUPPORTED.flatMap((code) =>
    CURRENCIES[code].stablecoins.map((s) => [s.symbol.toUpperCase(), s.symbol]),
  ),
);

const CURRENCY_BY_SYMBOL: Record<string, IsoCurrencyCode> = Object.fromEntries(
  SUPPORTED.flatMap((code) =>
    CURRENCIES[code].stablecoins.map((s) => [s.symbol, code]),
  ),
);

export const defaultStablecoin = (code: IsoCurrencyCode): string =>
  CURRENCIES[code].stablecoins[0].symbol;

export const currencyName = (code: IsoCurrencyCode): string =>
  NAME_BY_CODE[code] ?? code;

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

export const currencyForStablecoin = (
  symbol: string,
): IsoCurrencyCode | undefined => CURRENCY_BY_SYMBOL[symbol];

// Resolve a URL slug to a (currency, stablecoin) pair. Accepts either a
// stablecoin symbol (case-insensitive, returned in canonical case) or an ISO
// currency code (expands to that currency's default stablecoin). Returns null
// for anything else.
export const resolveTokenSlug = (
  raw: string | null | undefined,
): { currency: IsoCurrencyCode; stablecoin: string } | null => {
  if (!raw) return null;
  const upper = raw.toUpperCase();
  const canonical = SYMBOL_BY_UPPER[upper];
  if (canonical) {
    return { currency: CURRENCY_BY_SYMBOL[canonical], stablecoin: canonical };
  }
  if ((CURRENCIES as Record<string, unknown>)[upper]) {
    const cc = upper as IsoCurrencyCode;
    return { currency: cc, stablecoin: defaultStablecoin(cc) };
  }
  return null;
};
