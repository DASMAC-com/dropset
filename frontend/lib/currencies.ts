import countries from "world-countries";
import data from "./currencies.json";

export type IsoCurrencyCode = keyof typeof data;
export type Issuer = {
  name: string[];
  url: string;
  socials?: { x?: string };
};
export type Stablecoin = {
  symbol: string;
  name: string;
  mint: string;
  mintSourceUrl?: string;
  decimals: number;
  icon: string;
  issuer: Issuer;
};
export type CurrencyEntry = {
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

const ALL_CURRENCY_CODES = (() => {
  const set = new Set<string>();
  for (const c of countries) {
    for (const code of Object.keys(c.currencies ?? {})) set.add(code);
  }
  return set;
})();

export const currencyStats = (): {
  represented: number;
  total: number;
  missing: number;
} => {
  const represented = SUPPORTED.filter((c) => ALL_CURRENCY_CODES.has(c)).length;
  const total = ALL_CURRENCY_CODES.size;
  return { represented, total, missing: total - represented };
};

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

// Path to a 4:3 flag SVG mirrored from flag-icons into public/flag-icons by
// scripts/copy-flags.mjs. We use SVGs instead of Unicode flag emoji because
// Windows' default emoji font (Segoe UI Emoji) renders regional-indicator
// pairs as plain letter pairs ("US", "GB") rather than as flags.
export const flagUrl = (cca2: string): string =>
  `/flag-icons/${cca2.toLowerCase()}.svg`;

// ISO 4217 currency codes are conventionally country-code + currency initial,
// so the first two letters give a usable cca2 (USD→US, EUR→EU, GBP→GB…).
export const currencyFlagUrl = (code: IsoCurrencyCode): string =>
  flagUrl(code.slice(0, 2));

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
