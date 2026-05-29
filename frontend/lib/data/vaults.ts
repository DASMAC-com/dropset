import {
  currencyFlagUrl,
  currencyForStablecoin,
  type IsoCurrencyCode,
} from "./currencies";
import vaultsData from "./vaults.json";

// Per-leader liquidity vaults, grouped by market (pair). This module is the
// single seam between the UI and the vault data source: today it parses a
// committed mock fixture (vaults.json), but every consumer reads through the
// exports below, so swapping in a real indexer fetch later is a one-file
// change with no component edits. See docs/architecture.md → Vault.

// As stored in vaults.json. `base`/`quote` are stablecoin symbols (e.g.
// "EURC", "USDC") resolved against currencies.ts at load.
export type VaultRaw = {
  vaultPubkey: string;
  leader: string;
  volume24h: number;
  fees24h: number;
  tvl: number;
  minLeaderSharePpm: number;
  frozen: boolean;
  outsideDepositsApproved: boolean;
};
export type MarketRaw = {
  marketPubkey: string;
  base: string;
  quote: string;
  vaults: VaultRaw[];
};

export type Vault = VaultRaw;

// A market with its pair resolved to ISO currency codes + flag URLs for
// rendering. `label` is "<base currency> / <quote symbol>", e.g. "EUR / USDC".
export type VaultMarket = {
  marketPubkey: string;
  base: string;
  quote: string;
  baseCurrency: IsoCurrencyCode;
  quoteCurrency: IsoCurrencyCode;
  baseFlagUrl: string;
  quoteFlagUrl: string;
  label: string;
  vaults: Vault[];
};

const resolveCurrency = (
  symbol: string,
  marketPubkey: string,
): IsoCurrencyCode => {
  const code = currencyForStablecoin(symbol);
  if (!code) {
    throw new Error(
      `vaults.json market ${marketPubkey} references stablecoin "${symbol}" that isn't in currencies.json — every vault pair must resolve to a known currency.`,
    );
  }
  return code;
};

export const VAULT_MARKETS: VaultMarket[] = (
  vaultsData as { markets: MarketRaw[] }
).markets.map((m) => {
  const baseCurrency = resolveCurrency(m.base, m.marketPubkey);
  const quoteCurrency = resolveCurrency(m.quote, m.marketPubkey);
  return {
    marketPubkey: m.marketPubkey,
    base: m.base,
    quote: m.quote,
    baseCurrency,
    quoteCurrency,
    baseFlagUrl: currencyFlagUrl(baseCurrency),
    quoteFlagUrl: currencyFlagUrl(quoteCurrency),
    label: `${baseCurrency} / ${m.quote}`,
    vaults: m.vaults,
  };
});

const sum = (vaults: Vault[], pick: (v: Vault) => number): number =>
  vaults.reduce((acc, v) => acc + pick(v), 0);

export const marketVolume24h = (m: VaultMarket): number =>
  sum(m.vaults, (v) => v.volume24h);
export const marketFees24h = (m: VaultMarket): number =>
  sum(m.vaults, (v) => v.fees24h);
export const marketTvl = (m: VaultMarket): number =>
  sum(m.vaults, (v) => v.tvl);
export const vaultCount = (m: VaultMarket): number => m.vaults.length;

// Annualized 24h fee yield to depositors, as a fraction (0.1234 = 12.34%).
// Returns null when TVL is zero so the UI can render an em dash rather than a
// divide-by-zero Infinity.
const DAYS_PER_YEAR = 365;
export const vaultApr24h = (v: Vault): number | null =>
  v.tvl > 0 ? (v.fees24h / v.tvl) * DAYS_PER_YEAR : null;

// TVL-weighted market APR — sum the fees and TVL across vaults, then divide
// once. This is the correct aggregate; a plain mean of per-vault APRs would
// over-weight tiny vaults with outlier yields.
export const marketApr24h = (m: VaultMarket): number | null => {
  const tvl = marketTvl(m);
  return tvl > 0 ? (marketFees24h(m) / tvl) * DAYS_PER_YEAR : null;
};
