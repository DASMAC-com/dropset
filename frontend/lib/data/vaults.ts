// cspell:ignore swissy
import {
  currencyFlagUrl,
  currencyForStablecoin,
  currencyName,
  type IsoCurrencyCode,
  tokenIconUrl,
} from "./currencies";
import vaultsData from "./vaults.json";

// Per-leader liquidity vaults, grouped by FX trading pair (e.g. EUR/USD). This
// module is the single seam between the UI and the vault data source: today it
// parses a committed mock fixture (vaults.json), but every consumer reads
// through the exports below, so swapping in a real indexer fetch later is a
// one-file change with no component edits. See docs/architecture.md → Vault.

// As stored in vaults.json. `base`/`quote` are stablecoin symbols (e.g.
// "EURC", "USDC") resolved against currencies.ts at load. Token order encodes
// the FX convention: USD is the quote for EUR/GBP/AUD (base = EURC, quote =
// USDC) and the base for JPY/CHF/CAD (base = USDC, quote = GYEN).
export type VaultRaw = {
  vaultPubkey: string;
  leader: string;
  volume24h: number;
  fees24h: number;
  tvl: number;
  // Current pooled reserves in whole token units. Their ratio fixes the
  // pro-rata basket for deposits/withdrawals — adding liquidity must expand
  // both legs in proportion so the vault's price isn't moved.
  baseReserve: number;
  quoteReserve: number;
  // Total outstanding depositor shares; a position's `shares / totalShares` is
  // its claim on the reserves (see docs/architecture.md → Depositor positions).
  totalShares: number;
  // The leader's own (non-SPL) stake, included in totalShares. Its ratio to
  // totalShares is the skin-in-the-game number the min_leader_share floor
  // guards (see docs/architecture.md → Skin-in-the-game floor).
  leaderShares: number;
  // Current value-per-share (`L / totalShares`), the FX-neutral skill index a
  // position's `entryVps` is marked against for "yield since open".
  vps: number;
  minLeaderSharePpm: number;
  frozen: boolean;
  outsideDepositsApproved: boolean;
};
export type MarketRaw = {
  marketPubkey: string;
  base: string;
  quote: string;
  // Mock signed 24h FX move of the pair (fraction). Drives the "incl. FX"
  // leg of position PnL; the real value would come from an FX oracle.
  fxMove24h: number;
  vaults: VaultRaw[];
};

export type Vault = VaultRaw;

// A single stablecoin market (e.g. EURC/USDC) with its pair resolved to ISO
// currency codes + flag/icon URLs. `label` is the stablecoin pair, e.g.
// "EURC / USDC".
export type VaultMarket = {
  marketPubkey: string;
  base: string;
  quote: string;
  baseCurrency: IsoCurrencyCode;
  quoteCurrency: IsoCurrencyCode;
  baseFlagUrl: string;
  quoteFlagUrl: string;
  baseIconUrl: string;
  quoteIconUrl: string;
  // Mock USD price per base / quote token, for the dialog's ≈ USD readouts.
  baseUsd: number;
  quoteUsd: number;
  label: string;
  fxMove24h: number;
  vaults: Vault[];
};

// One vault paired with the market it trades — the atomic row of the table.
export type GroupedVault = { market: VaultMarket; vault: Vault };

// An FX trading pair (e.g. EUR / USD) grouping every vault whose market's base
// and quote resolve to the same two currencies. Carries the summed aggregates
// shown on the group header row.
export type FxPairGroup = {
  key: string;
  baseCurrency: IsoCurrencyCode;
  quoteCurrency: IsoCurrencyCode;
  baseFlagUrl: string;
  quoteFlagUrl: string;
  baseName: string;
  quoteName: string;
  label: string;
  nickname: string;
  vaults: GroupedVault[];
  volume24h: number;
  fees24h: number;
  tvl: number;
  apr24h: number | null;
};

// FX dealer nicknames for the majors we list (NZD/"Kiwi" excluded for now).
const FX_NICKNAMES: Record<string, string> = {
  "EUR/USD": "Fiber",
  "USD/JPY": "Ninja",
  "GBP/USD": "Cable",
  "USD/CHF": "Swissy",
  "USD/CAD": "Loonie",
  "AUD/USD": "Aussie",
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

// Hard-coded mock USD prices per token symbol, used only for the dialog's
// approximate USD readouts (the quote leg isn't always a USD stable, e.g.
// USD/JPY quotes GYEN). Swap for a real oracle / price feed later.
const TOKEN_USD: Record<string, number> = {
  USDC: 1,
  USDT: 1,
  EURC: 1.08,
  VEUR: 1.08,
  GYEN: 0.0067,
  TGBP: 1.27,
  VGBP: 1.27,
  VCHF: 1.12,
  CADC: 0.73,
  AUDD: 0.66,
};

// USD price for a stablecoin symbol; defaults to 1 (a USD stable) when unknown.
export const tokenUsdPrice = (symbol: string): number => TOKEN_USD[symbol] ?? 1;

const VAULT_MARKETS: VaultMarket[] = (
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
    baseIconUrl: tokenIconUrl(m.base),
    quoteIconUrl: tokenIconUrl(m.quote),
    baseUsd: tokenUsdPrice(m.base),
    quoteUsd: tokenUsdPrice(m.quote),
    label: `${m.base} / ${m.quote}`,
    fxMove24h: m.fxMove24h,
    vaults: m.vaults,
  };
});

// Annualized 24h fee yield to depositors, as a fraction (0.1234 = 12.34%).
// Returns null when TVL is zero so the UI can render an em dash rather than a
// divide-by-zero Infinity.
const DAYS_PER_YEAR = 365;
const annualizedYield = (fees24h: number, tvl: number): number | null =>
  tvl > 0 ? (fees24h / tvl) * DAYS_PER_YEAR : null;

export const vaultApr24h = (v: Vault): number | null =>
  annualizedYield(v.fees24h, v.tvl);

// Quote tokens per base token in the vault's current reserves. Deposits and
// withdrawals must preserve this ratio (a pro-rata basket), so the UI fixes
// one leg and derives the other from it. Returns null for an empty vault
// (zero reserves), where there's no ratio to preserve.
export const vaultReserveRatio = (v: Vault): number | null =>
  v.baseReserve > 0 && v.quoteReserve > 0
    ? v.quoteReserve / v.baseReserve
    : null;

// The min_leader_share floor as a fraction (the fixture stores it in ppm,
// 1_000_000 = 100%).
export const leaderFloorFraction = (v: Vault): number =>
  v.minLeaderSharePpm / 1_000_000;

// The leader's current stake as a fraction of all shares — their live
// skin-in-the-game ratio. null for an empty vault (no shares yet).
export const leaderShareFraction = (v: Vault): number | null =>
  v.totalShares > 0 ? v.leaderShares / v.totalShares : null;

// Largest outside deposit (quote-denominated value) the vault can accept
// before the leader's stake would dilute below its min_leader_share floor.
// A deposit mints `value / vps` new shares at the current value-per-share
// while the leader's stake stays fixed, so the floor binds when
//   leader_shares / (total_shares + value / vps) >= floor.
// Solving for value gives the cap. Returns null when the floor can't bind —
// no recorded leader stake or a zero-vps / empty vault — in which case the
// frozen / outside-deposits-approved gates govern instead.
export const maxOutsideDepositValue = (v: Vault): number | null => {
  const floor = leaderFloorFraction(v);
  if (v.leaderShares <= 0 || v.vps <= 0 || floor <= 0) return null;
  const maxTotalShares = v.leaderShares / floor;
  const room = maxTotalShares - v.totalShares;
  return room > 0 ? room * v.vps : 0;
};

// Group vaults into FX pairs, preserving first-seen order of both the groups
// and the vaults within. Derived from the resolved currency codes so USDC- and
// USDT-quoted markets fold into one "EUR / USD" group. Pair-level metrics sum
// across every vault; APR is TVL-weighted (total fees / total TVL).
export const VAULT_FX_GROUPS: FxPairGroup[] = (() => {
  const order: string[] = [];
  const groups = new Map<string, FxPairGroup>();
  for (const m of VAULT_MARKETS) {
    const key = `${m.baseCurrency}/${m.quoteCurrency}`;
    let group = groups.get(key);
    if (!group) {
      group = {
        key,
        baseCurrency: m.baseCurrency,
        quoteCurrency: m.quoteCurrency,
        baseFlagUrl: m.baseFlagUrl,
        quoteFlagUrl: m.quoteFlagUrl,
        baseName: currencyName(m.baseCurrency),
        quoteName: currencyName(m.quoteCurrency),
        label: `${m.baseCurrency} / ${m.quoteCurrency}`,
        nickname: FX_NICKNAMES[`${m.baseCurrency}/${m.quoteCurrency}`] ?? "",
        vaults: [],
        volume24h: 0,
        fees24h: 0,
        tvl: 0,
        apr24h: null,
      };
      groups.set(key, group);
      order.push(key);
    }
    for (const vault of m.vaults) {
      group.vaults.push({ market: m, vault });
      group.volume24h += vault.volume24h;
      group.fees24h += vault.fees24h;
      group.tvl += vault.tvl;
    }
  }
  for (const group of groups.values()) {
    group.apr24h = annualizedYield(group.fees24h, group.tvl);
  }
  return order.map((key) => {
    const g = groups.get(key);
    if (!g) throw new Error(`unreachable: group ${key} missing`);
    return g;
  });
})();

// Flat list of every vault across all pairs, for the ungrouped table view.
export const ALL_VAULTS: GroupedVault[] = VAULT_FX_GROUPS.flatMap(
  (g) => g.vaults,
);

// Sortable numeric metrics. APR can be null (zero TVL); callers push nulls to
// the bottom regardless of sort direction. "position" is the connected user's
// position value — it depends on wallet state, so vaultMetric/groupMetric
// return null for it and the view supplies the value.
export type MetricKey =
  | "apr24h"
  | "tvl"
  | "volume24h"
  | "position"
  | "leader"
  | "pair";

export const vaultMetric = (
  gv: GroupedVault,
  key: MetricKey,
): number | null => {
  if (key === "apr24h") return vaultApr24h(gv.vault);
  if (key === "tvl") return gv.vault.tvl;
  if (key === "volume24h") return gv.vault.volume24h;
  return null;
};

export const groupMetric = (g: FxPairGroup, key: MetricKey): number | null => {
  if (key === "apr24h") return g.apr24h;
  if (key === "tvl") return g.tvl;
  if (key === "volume24h") return g.volume24h;
  return null;
};
