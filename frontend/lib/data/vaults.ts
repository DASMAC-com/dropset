// cspell:ignore swissy
import {
  currencyFlagUrl,
  currencyForStablecoin,
  currencyName,
  type IsoCurrencyCode,
} from "./currencies";
import vaultsData from "./vaults.json";

// Per-leader liquidity vaults, grouped by FX trading pair (e.g. EUR/USD). Two
// sources share the shapes below: the live vaults read straight off each
// market account (lib/data/chainVaults.ts, no indexer), and the committed mock
// fixture (vaults.json), shown only behind NEXT_PUBLIC_SHOW_MOCK_VAULTS. The
// fixture's pair list doubles as the roster of markets the live read probes.
// See docs/architecture.md → Vault.

// As stored in vaults.json. `base`/`quote` are stablecoin symbols (e.g.
// "EURC", "USDC") resolved against currencies.ts at load. Token order encodes
// the FX convention: USD is the quote for EUR/GBP/AUD (base = EURC, quote =
// USDC) and the base for JPY/CHF/CAD (base = USDC, quote = GYEN).
export type VaultRaw = {
  // The vault's id: a pubkey on a mock row, but `market:seq` on a live one
  // (chainVaultId), since a live vault is a market sector with no account of
  // its own. Use it only as a key, never as an address.
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
  // position's `entryVps` is marked against for "yield since open". Only its
  // ratio to `entryVps` means anything: a mock row is normalized near 1.0, a
  // live row is in the program's raw units (chainVaults.ts → chainVps).
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

export type Vault = Omit<VaultRaw, "volume24h" | "fees24h"> & {
  // A live vault's on-chain identity, null on a mock row. `index` is the
  // sector the deposit / withdraw instructions and the depositor PDA take;
  // `seq` is the stable per-market vault number. The raw atom and share
  // counts, and the fee state a withdraw realizes first, are what the
  // dialog's share preview sizes against, so the SDK's integer kernels see
  // exactly what the program will.
  chain: {
    index: number;
    seq: bigint;
    baseAtoms: bigint;
    quoteAtoms: bigint;
    totalShares: bigint;
    leaderShares: bigint;
    hwm: bigint;
    perfFeeRatePpm: number;
    frozen: boolean;
  } | null;
  // Null on a live vault: 24h volume and fees need history, which no single
  // account read can give.
  volume24h: number | null;
  fees24h: number | null;
  // The leader's opt-in to outside depositors. Outside deposits need it, the
  // admin's `outsideDepositsApproved`, and an unfrozen vault — see depositsOpen.
  allowOutsideDepositors: boolean;
  // The vault's display name. Always null until the program stores one; the
  // dialog already renders it when present.
  name: string | null;
};

// The fields depositClosedReason reads, so it gates a freshly decoded
// VaultSummary (raw counts) as readily as a table row.
type DepositGate = Pick<
  Vault,
  "frozen" | "allowOutsideDepositors" | "outsideDepositsApproved"
> & { leader: string; totalShares: number | bigint };

// Why an outside deposit into `v` is closed, or null when it is open. Mirrors
// the program's own gate in `deposit`, in its order — frozen, the signer being
// the leader, an unseeded vault, the leader's opt-in, the admin's approval —
// so the page never offers a deposit it would reject. `signer` is the
// connected wallet, if any; with none, the leader check is skipped.
export const depositClosedReason = (
  v: DepositGate,
  signer?: string | null,
): string | null => {
  if (v.frozen) return "This vault is frozen, so deposits are closed.";
  if (signer && signer === v.leader)
    return "You lead this vault. Leader deposits aren't made from this page.";
  if (Number(v.totalShares) <= 0)
    return "Not open yet: the leader hasn't funded this vault.";
  if (!v.allowOutsideDepositors)
    return "Not open yet: the leader hasn't opened this vault to outside deposits.";
  if (!v.outsideDepositsApproved)
    return "Not open yet: outside deposits are waiting on approval.";
  return null;
};

// Why the contribute surface takes no deposit into `v`, or null when it does:
// the program's gates first (depositClosedReason), then two states the
// program would size but the surface holds closed, in words. A one-sided
// vault (one leg empty) would only take the leg it holds, so it waits for the
// book to rebalance; a vault at its leader floor has no room for any outside
// capital.
export const contributeClosedReason = (
  market: Pick<VaultMarket, "base" | "quote">,
  v: Vault,
  signer?: string | null,
): string | null => {
  const gate = depositClosedReason(v, signer);
  if (gate) return gate;
  if (v.baseReserve <= 0 || v.quoteReserve <= 0) {
    const held = v.baseReserve > 0 ? market.base : market.quote;
    return `One-sided for now: the vault holds only ${held}. Deposits reopen when the book rebalances.`;
  }
  if (maxOutsideDepositValue(v) === 0) {
    const floor = leaderFloorFraction(v);
    const multiple = Math.round((1 - floor) / floor);
    const floorPct = Math.round(floor * 100);
    return `Full: outside capital is at its cap of ${multiple} times the leader's stake (the leader keeps at least ${floorPct}% of the vault).`;
  }
  return null;
};

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
  volume24h: number | null;
  fees24h: number | null;
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
      `vault market ${marketPubkey} references stablecoin "${symbol}" that isn't in currencies.json — every vault pair must resolve to a known currency.`,
    );
  }
  return code;
};

// A market with its pair resolved to ISO currencies and flags — shared by the
// mock loader and the live read, which differ only in where `vaults` came from.
export const resolveVaultMarket = (
  marketPubkey: string,
  base: string,
  quote: string,
  fxMove24h: number,
  vaults: Vault[],
): VaultMarket => {
  const baseCurrency = resolveCurrency(base, marketPubkey);
  const quoteCurrency = resolveCurrency(quote, marketPubkey);
  return {
    marketPubkey,
    base,
    quote,
    baseCurrency,
    quoteCurrency,
    baseFlagUrl: currencyFlagUrl(baseCurrency),
    quoteFlagUrl: currencyFlagUrl(quoteCurrency),
    label: `${base} / ${quote}`,
    fxMove24h,
    vaults,
  };
};

// Hard-coded mock USD prices per token symbol, used only for the dialog's
// approximate USD readouts (the quote leg isn't always a USD stable, e.g.
// USD/JPY quotes GYEN). Swap for a real oracle / price feed later.
const TOKEN_USD: Record<string, number> = {
  USDC: 1,
  USDT: 1,
  EURC: 1.08,
  GYEN: 0.0067,
  TGBP: 1.27,
  VGBP: 1.27,
  VCHF: 1.12,
  CADC: 0.73,
  AUDD: 0.66,
};

// USD price for a stablecoin symbol; defaults to 1 (a USD stable) when unknown.
export const tokenUsdPrice = (symbol: string): number => TOKEN_USD[symbol] ?? 1;

// The fixture predates the leader opt-in flag, so a mock vault reads the
// admin approval as both halves of the gate — the two only differ on-chain.
export const MOCK_VAULT_MARKETS: VaultMarket[] = (
  vaultsData as { markets: MarketRaw[] }
).markets.map((m) =>
  resolveVaultMarket(
    m.marketPubkey,
    m.base,
    m.quote,
    m.fxMove24h,
    m.vaults.map((v) => ({
      ...v,
      chain: null,
      allowOutsideDepositors: v.outsideDepositsApproved,
      name: null,
    })),
  ),
);

// The stablecoin pairs the live read probes for a market, in the fixture's
// orientation. The live read takes each market's real orientation from the
// chain, so this order is only the roster's.
export const VAULT_PAIR_ROSTER: { base: string; quote: string }[] =
  MOCK_VAULT_MARKETS.map((m) => ({ base: m.base, quote: m.quote }));

// The vault market for an unordered stablecoin pair. A vault's base/quote are
// fixed by the market, not by which way a taker trades — so USDC→EURC and
// EURC→USDC both map to the one EURC/USDC market. Returns null when no vault
// lists the pair (callers use this to hide a "view vaults" affordance).
export const findVaultMarket = (a: string, b: string): VaultMarket | null =>
  MOCK_VAULT_MARKETS.find(
    (m) => (m.base === a && m.quote === b) || (m.base === b && m.quote === a),
  ) ?? null;

// The live vault a public vault link names (vaultLinkHref): the market with
// this base / quote, and the vault in it with this `seq`. `seq` is the raw
// query value, so anything but a decimal integer matches nothing.
export const findVaultBySeq = (
  markets: VaultMarket[],
  base: string,
  quote: string,
  seq: string,
): GroupedVault | null => {
  if (!/^\d+$/.test(seq)) return null;
  const want = BigInt(seq);
  for (const market of markets) {
    if (market.base !== base || market.quote !== quote) continue;
    const vault = market.vaults.find((v) => v.chain?.seq === want);
    if (vault) return { market, vault };
  }
  return null;
};

// Annualized 24h fee yield to depositors, as a fraction (0.1234 = 12.34%).
// Returns null when TVL is zero so the UI can render an em dash rather than a
// divide-by-zero Infinity.
const DAYS_PER_YEAR = 365;
const annualizedYield = (fees24h: number | null, tvl: number): number | null =>
  fees24h !== null && tvl > 0 ? (fees24h / tvl) * DAYS_PER_YEAR : null;

// Sum of 24h figures that may be unknown: null only when every term is, so a
// group mixing live and mock rows still totals the rows that have a figure.
const sumKnown = (a: number | null, b: number | null): number | null =>
  a === null ? b : b === null ? a : a + b;

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

// Parts-per-million is how the protocol stores share fractions (1_000_000 =
// 100%); see Ppm32 in docs/architecture.md.
const PPM = 1_000_000;

// The min_leader_share floor as a fraction.
export const leaderFloorFraction = (v: Vault): number =>
  v.minLeaderSharePpm / PPM;

// The leader's current stake as a fraction of all shares — their live
// skin-in-the-game ratio. null for an empty vault (no shares yet).
export const leaderShareFraction = (v: Vault): number | null =>
  v.totalShares > 0 ? v.leaderShares / v.totalShares : null;

// The vault's current QUOTE-denominated value per share: the pooled reserves
// valued in quote tokens (`baseReserve · ratio + quoteReserve`) over total
// shares. This is the unit a deposit's quote value is minted against — distinct
// from `Vault.vps`, an FX-neutral skill index read only as a ratio to a
// position's `entryVps` for "yield since open". null for an empty vault.
export const vaultValuePerShare = (v: Vault): number | null => {
  const ratio = vaultReserveRatio(v);
  if (ratio === null || v.totalShares <= 0) return null;
  return (v.baseReserve * ratio + v.quoteReserve) / v.totalShares;
};

// Largest outside deposit (quote-denominated value) the vault can accept
// before the leader's stake would dilute below its min_leader_share floor.
// A deposit of quote value V mints `V / valuePerShare` new shares while the
// leader's stake stays fixed, so the floor binds when
//   leader_shares / (total_shares + V / valuePerShare) >= floor.
// Solving for V gives the cap. Uses the QUOTE-denominated value per share so
// the cap is in the same units as the entered basket (critical for non-USD
// quote legs like GYEN); using the normalized `vps` here understated the cap
// and falsely tripped the floor warning on safe deposits. Returns null when the
// floor can't bind (no leader stake / empty vault) — the frozen /
// outside-deposits-approved gates govern instead.
export const maxOutsideDepositValue = (v: Vault): number | null => {
  const floor = leaderFloorFraction(v);
  const valuePerShare = vaultValuePerShare(v);
  if (v.leaderShares <= 0 || valuePerShare === null || floor <= 0) return null;
  const maxTotalShares = v.leaderShares / floor;
  const room = maxTotalShares - v.totalShares;
  return room > 0 ? room * valuePerShare : 0;
};

// Mock outside-depositor wallet balance per leg: ~2% of each pooled reserve, so
// Max / a percent fills a plausible pro-rata basket. This is the seam for real
// balances — the wallet integration replaces this one helper, not dialog code.
const MOCK_DEPOSIT_FRACTION = 0.02;
export const mockMaxDeposit = (v: Vault): { base: number; quote: number } => ({
  base: v.baseReserve * MOCK_DEPOSIT_FRACTION,
  quote: v.quoteReserve * MOCK_DEPOSIT_FRACTION,
});

// Group vaults into FX pairs, preserving first-seen order of both the groups
// and the vaults within. Derived from the resolved currency codes so USDC- and
// USDT-quoted markets fold into one "EUR / USD" group. Pair-level metrics sum
// across every vault; APR is TVL-weighted (total fees / total TVL).
export const groupVaultMarkets = (markets: VaultMarket[]): FxPairGroup[] => {
  const order: string[] = [];
  const groups = new Map<string, FxPairGroup>();
  for (const m of markets) {
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
        volume24h: null,
        fees24h: null,
        tvl: 0,
        apr24h: null,
      };
      groups.set(key, group);
      order.push(key);
    }
    for (const vault of m.vaults) {
      group.vaults.push({ market: m, vault });
      group.volume24h = sumKnown(group.volume24h, vault.volume24h);
      group.fees24h = sumKnown(group.fees24h, vault.fees24h);
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
};

export const MOCK_VAULT_FX_GROUPS: FxPairGroup[] =
  groupVaultMarkets(MOCK_VAULT_MARKETS);

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
