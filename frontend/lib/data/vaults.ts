// cspell:ignore swissy
import {
  currencyFlagUrl,
  currencyForStablecoin,
  currencyName,
  type IsoCurrencyCode,
} from "./currencies";

// Per-leader liquidity vaults, grouped by FX trading pair (e.g. EUR/USD), read
// straight off each market account (lib/data/chainVaults.ts, no indexer). See
// docs/architecture.md → Vault.

export type Vault = {
  // The vault's id, `market:seq` (chainVaultId), since a vault is a market
  // sector with no account of its own. Use it only as a key, never as an
  // address.
  vaultPubkey: string;
  leader: string;
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
  // ratio to `entryVps` means anything; it is in the program's raw units
  // (chainVaults.ts → chainVps).
  vps: number;
  minLeaderSharePpm: number;
  frozen: boolean;
  outsideDepositsApproved: boolean;
  // The vault's on-chain identity. `index` is the sector the deposit /
  // withdraw instructions and the depositor PDA take; `seq` is the stable
  // per-market vault number. The raw atom and share counts, and the fee state
  // a withdraw realizes first, are what the dialog's share preview sizes
  // against, so the SDK's integer kernels see exactly what the program will.
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
  };
  // The vault's live reference price in whole quote tokens per base token,
  // read off its own sector. Null while the book is dark (a non-matchable
  // price); see vaultRefPrice for the stand-in.
  refPrice: number | null;
  // USD per quote token, and whether that rate (or TVL's base leg) fell back
  // to the indicative TOKEN_USD table rather than an on-chain price. See
  // quoteUsdRate.
  quoteUsd: number;
  usdIndicative: boolean;
  // The leader's opt-in to outside depositors. Outside deposits need it, the
  // admin's `outsideDepositsApproved`, and an unfrozen vault — see depositsOpen.
  allowOutsideDepositors: boolean;
  // The vault's display name, read from the vault account; null when unset
  // (never named, or blanked by freeze or close). The table falls back to
  // the vault number (see vaultLabel).
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

// Parts-per-million is how the protocol stores share fractions (1_000_000 =
// 100%); see Ppm32 in docs/architecture.md.
export const PPM = 1_000_000n;

// Whether a deposit of `sharesOut` keeps the leader at or above their
// min_leader_share floor — the program's own integer check, so it decides
// exactly where the dialog's float warning only estimates. The table's "Full"
// state and the send-time gate share it, so a vault the gate would refuse
// even one share never reads open (atLeaderFloor closes a sliver earlier).
export function leaderFloorHolds(
  vault: {
    leaderShares: bigint;
    totalShares: bigint;
    minLeaderSharePpm: number;
  },
  sharesOut: bigint,
): boolean {
  const totalAfter = vault.totalShares + sharesOut;
  return (
    vault.leaderShares * PPM >= BigInt(vault.minLeaderSharePpm) * totalAfter
  );
}

// The smallest room, in USD, the contribute surface treats as open. Below it
// the vault reads full: any real deposit would breach the leader floor at send
// time, so offering the form would only invite a rejection.
export const MIN_CONTRIBUTE_ROOM_USD = 1;

// Whether the vault has less than one real deposit of room under the leader's
// min_leader_share floor: not even one more share fits (the program's own
// integer check, so a vault exactly at its floor reads full), or the room left
// is worth under MIN_CONTRIBUTE_ROOM_USD.
const atLeaderFloor = (v: Vault): boolean => {
  if (
    !leaderFloorHolds(
      { ...v.chain, minLeaderSharePpm: v.minLeaderSharePpm },
      1n,
    )
  )
    return true;
  const room = maxOutsideDepositValue(v);
  return room !== null && room * v.quoteUsd < MIN_CONTRIBUTE_ROOM_USD;
};

// Why the contribute surface takes no deposit into `v`, or null when it does,
// in words: the program's gates first (depositClosedReason), then an empty
// or one-sided vault, which the program would size but the surface holds
// closed (a one-sided vault would only take the leg it holds, so it waits for
// the book to rebalance), then a vault at (or a sliver short of) its leader
// floor, where the program would reject any real outside deposit.
export const contributeClosedReason = (
  market: Pick<VaultMarket, "base" | "quote">,
  v: Vault,
  signer?: string | null,
): string | null => {
  const gate = depositClosedReason(v, signer);
  if (gate) return gate;
  if (v.baseReserve <= 0 && v.quoteReserve <= 0)
    return "Closed for now: the vault holds neither asset.";
  if (v.baseReserve <= 0 || v.quoteReserve <= 0) {
    const held = v.baseReserve > 0 ? market.base : market.quote;
    return `One-sided for now: the vault holds only ${held}. Deposits reopen when the book rebalances.`;
  }
  if (atLeaderFloor(v)) {
    const floor = leaderFloorFraction(v);
    const multiple = Math.round((1 - floor) / floor);
    const floorPct = Math.round(floor * 100);
    return `Full: outside capital is at its cap of ${multiple} times the leader's stake (the leader keeps at least ${floorPct}% of the vault).`;
  }
  return null;
};

// A single stablecoin market (e.g. EURC/USDC) with its pair resolved to ISO
// currency codes + flag/icon URLs. `base`/`quote` are stablecoin symbols (e.g.
// "EURC", "USDC") resolved against currencies.ts; `label` is the stablecoin
// pair, e.g. "EURC / USDC".
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
  tvl: number;
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

// A market with its pair resolved to ISO currencies and flags.
export const resolveVaultMarket = (
  marketPubkey: string,
  base: string,
  quote: string,
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
    vaults,
  };
};

// Hard-coded indicative USD prices per token symbol: the fallback when a vault
// has no on-chain price to value a leg at (a dark book, or a pair with no USD
// leg).
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

const isUsd = (symbol: string): boolean =>
  currencyForStablecoin(symbol) === "USD";

// USD per quote token for a market wherever one leg is a USD stable: 1 for a
// USD-quoted market, and 1 / refPrice for a USD-based one (USD/JPY quotes
// GYEN), from the vault's own reference price `refPrice` (quote per base).
// Every roster pair has a USD leg, so the table is reached only by a dark
// book on a USD-based market, or a cross pair; `indicative` says so.
export const quoteUsdRate = (
  base: string,
  quote: string,
  refPrice: number | null,
): { rate: number; indicative: boolean } => {
  if (isUsd(quote)) return { rate: 1, indicative: false };
  if (isUsd(base) && refPrice !== null && refPrice > 0)
    return { rate: 1 / refPrice, indicative: false };
  return { rate: tokenUsdPrice(quote), indicative: true };
};

// A stablecoin pair by symbol, e.g. { base: "EURC", quote: "USDC" }.
export type VaultPair = { base: string; quote: string };

// The stablecoin pairs whose markets the live read probes. Token order
// encodes the FX convention: USD is the quote for EUR/GBP/AUD (base = EURC,
// quote = USDC) and the base for JPY/CHF/CAD (base = USDC, quote = GYEN). The
// live read takes each market's real orientation from the chain, so this
// order is only the roster's. Every symbol must be in currencies.json
// (vaults.test.ts).
export const VAULT_PAIR_ROSTER: VaultPair[] = [
  { base: "EURC", quote: "USDC" },
  { base: "EURC", quote: "USDT" },
  { base: "USDC", quote: "GYEN" },
  { base: "TGBP", quote: "USDC" },
  { base: "VGBP", quote: "USDC" },
  { base: "USDC", quote: "VCHF" },
  { base: "USDC", quote: "CADC" },
  { base: "AUDD", quote: "USDC" },
];

// The roster pair for an unordered stablecoin pair. A vault's base/quote are
// fixed by the market, not by which way a taker trades — so USDC→EURC and
// EURC→USDC both map to the one EURC/USDC pair. Returns null when the roster
// lists no such pair (callers use this to hide a "view vaults" affordance).
export const findVaultPair = (a: string, b: string): VaultPair | null =>
  VAULT_PAIR_ROSTER.find(
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
    const vault = market.vaults.find((v) => v.chain.seq === want);
    if (vault) return { market, vault };
  }
  return null;
};

// The Vaults tab's structured filter: a market (base/quote symbols) and an
// optional vault number (`seq`, decimal), each empty when unset. A vault
// number is per-market, so it means something only beside its pair. `leader`
// is the pre-number key (LEGACY_LEADER_PARAM), only ever read from an old
// link: a case-insensitive prefix filter until it resolves to a vault number
// (resolveLegacyLeaderPin). One-release fallback; drop it with the param.
export type VaultPin = {
  base: string;
  quote: string;
  vault: string;
  leader: string;
};

// Whether a vault passes the pin: exact market on base/quote (when set), the
// exact vault number, and the legacy leader prefix.
export const vaultPinMatches = (
  pin: VaultPin,
  market: Pick<VaultMarket, "base" | "quote">,
  vault: Vault,
): boolean =>
  (!pin.base || market.base === pin.base) &&
  (!pin.quote || market.quote === pin.quote) &&
  (!pin.vault || vault.chain.seq.toString() === pin.vault) &&
  (!pin.leader ||
    vault.leader.toLowerCase().startsWith(pin.leader.toLowerCase()));

// Whether the pin names exactly this vault (the row's pin toggle).
export const isVaultPinned = (
  pin: VaultPin,
  market: Pick<VaultMarket, "base" | "quote">,
  vault: Vault,
): boolean =>
  pin.base === market.base &&
  pin.quote === market.quote &&
  pin.vault === vault.chain.seq.toString();

// The vault number an old leader-keyed pin (LEGACY_LEADER_PARAM) named: the
// one vault in the base / quote market whose leader the pin prefixes,
// case-insensitively, as the old filter matched. Null when none or several
// match — a rotated leader names nothing, and an ambiguous prefix is left to
// filter as it did. One-release fallback; drop it with the param.
export const resolveLegacyLeaderPin = (
  markets: VaultMarket[],
  base: string,
  quote: string,
  leader: string,
): bigint | null => {
  if (!leader) return null;
  const want = leader.toLowerCase();
  const hits = markets
    .filter((m) => m.base === base && m.quote === quote)
    .flatMap((m) => m.vaults)
    .filter((v) => v.leader.toLowerCase().startsWith(want));
  return hits.length === 1 ? (hits[0]?.chain.seq ?? null) : null;
};

// Quote tokens per base token in the vault's current reserves. Deposits and
// withdrawals must preserve this ratio (a pro-rata basket), so the UI fixes
// one leg and derives the other from it. Returns null for an empty vault
// (zero reserves), where there's no ratio to preserve.
export const vaultReserveRatio = (v: Vault): number | null =>
  v.baseReserve > 0 && v.quoteReserve > 0
    ? v.quoteReserve / v.baseReserve
    : null;

// The price a position is marked at (PnL's `refNow`), quote per base: the
// vault's live reference price, else its reserve ratio as the stand-in (a
// dark book).
export const vaultRefPrice = (v: Vault): number | null =>
  v.refPrice ?? vaultReserveRatio(v);

// The min_leader_share floor as a fraction.
export const leaderFloorFraction = (v: Vault): number =>
  v.minLeaderSharePpm / Number(PPM);

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

// Group vaults into FX pairs, preserving first-seen order of both the groups
// and the vaults within. Derived from the resolved currency codes so USDC- and
// USDT-quoted markets fold into one "EUR / USD" group. The pair's TVL sums
// across every vault.
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
        tvl: 0,
      };
      groups.set(key, group);
      order.push(key);
    }
    for (const vault of m.vaults) {
      group.vaults.push({ market: m, vault });
      group.tvl += vault.tvl;
    }
  }
  return order.map((key) => {
    const g = groups.get(key);
    if (!g) throw new Error(`unreachable: group ${key} missing`);
    return g;
  });
};

// Sortable metrics. "position" is the connected user's position value — it
// depends on wallet state, so vaultMetric/groupMetric return null for it and
// the view supplies the value.
export type MetricKey = "tvl" | "position" | "name" | "pair";

// The vault number every user surface names a vault by: `Vault #<seq>`, the
// per-market number, as the TUI shows it. Never the sector index, which is
// addressing (reused once a vault closes), nor the leader, which rotates.
export const vaultNumberLabel = (vault: Vault): string =>
  `Vault #${vault.chain.seq}`;

// The table's handle for a vault, and its "name" sort value: its name, else
// its vault number.
export const vaultLabel = (vault: Vault): string =>
  vault.name ?? vaultNumberLabel(vault);

export const vaultMetric = (gv: GroupedVault, key: MetricKey): number | null =>
  key === "tvl" ? gv.vault.tvl : null;

export const groupMetric = (g: FxPairGroup, key: MetricKey): number | null =>
  key === "tvl" ? g.tvl : null;
