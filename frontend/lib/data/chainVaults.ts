import {
  decodeDropsetVaults,
  decodePrice,
  fetchAllMaybeVaultDepositorHeader,
  findVaultDepositorPda,
  initSimulator,
  type VaultDepositorHeader,
  type VaultSummary,
} from "@dropset/sdk";
import type { SolanaClientRuntime } from "@solana/client";
import { address } from "@solana/kit";
import { resolveEclobRoute } from "../eclob/route";
import { stablecoinDecimals, stablecoinMint } from "./currencies";
import type { VaultPosition } from "./positions";
import {
  resolveVaultMarket,
  tokenUsdPrice,
  VAULT_PAIR_ROSTER,
  type Vault,
  type VaultMarket,
  vaultValuePerShare,
} from "./vaults";

// The live half of the vaults page: every vault on every roster market, read
// straight off the market accounts (one account per market, no indexer), and
// the connected wallet's positions read off its VaultDepositor PDAs.
//
// The UI's Vault / VaultPosition shapes are whole-token floats, so this is
// where atoms meet display units. Shares stay raw counts: every consumer uses
// them only as a ratio to totalShares, and both sides of that ratio are raw.

type Rpc = SolanaClientRuntime["rpc"];

// Value-per-share and the entry VPS are Q32.32 on-chain.
const Q32 = 2 ** 32;

const atomsToUnits = (atoms: bigint, decimals: number): number =>
  Number(atoms) / 10 ** decimals;

// A live vault's id. There is no vault account — a vault is a sector of its
// market — so the row key and the position lookup use (market, seq), the
// pair `seq` is documented to identify.
export const chainVaultId = (market: string, seq: bigint): string =>
  `${market}:${seq}`;

// One decoded vault in the UI's shape. TVL is valued at the same placeholder
// token prices as the dialog's USD readouts (tokenUsdPrice), and the 24h
// figures are null: they need history a single account read cannot give.
export function chainVault(
  summary: VaultSummary,
  market: string,
  base: string,
  quote: string,
): Vault {
  const baseReserve = atomsToUnits(summary.baseAtoms, stablecoinDecimals(base));
  const quoteReserve = atomsToUnits(
    summary.quoteAtoms,
    stablecoinDecimals(quote),
  );
  const vault: Vault = {
    chain: { index: summary.index, seq: summary.seq },
    vaultPubkey: chainVaultId(market, summary.seq),
    leader: summary.leader,
    volume24h: null,
    fees24h: null,
    tvl:
      baseReserve * tokenUsdPrice(base) + quoteReserve * tokenUsdPrice(quote),
    baseReserve,
    quoteReserve,
    totalShares: Number(summary.totalShares),
    leaderShares: Number(summary.leaderShares),
    vps: 0,
    minLeaderSharePpm: summary.minLeaderSharePpm,
    frozen: summary.frozen,
    allowOutsideDepositors: summary.allowOutsideDepositors,
    outsideDepositsApproved: summary.outsideDepositsApproved,
    name: null,
  };
  // Marked at the reserve ratio, the same stand-in the PnL readouts use for
  // the live reference price, in the units `chainPosition` gives entryVps.
  vault.vps = vaultValuePerShare(vault) ?? 0;
  return vault;
}

// A VaultDepositor account in the UI's position shape. Quote-denominated
// figures are quote atoms on-chain; the entry reference price is an
// atoms ratio, rescaled to whole tokens by the decimals gap.
export function chainPosition(
  header: VaultDepositorHeader,
  vaultId: string,
  base: string,
  quote: string,
): VaultPosition {
  const quoteDecimals = stablecoinDecimals(quote);
  const quoteUnits = (atoms: bigint) => atomsToUnits(atoms, quoteDecimals);
  return {
    vaultPubkey: vaultId,
    owner: header.owner,
    shares: Number(header.shares),
    netDeposits: quoteUnits(header.netDeposits),
    grossDeposited: quoteUnits(header.grossDeposited),
    entryRefPrice:
      decodePrice(header.entryRefPrice) *
      10 ** (stablecoinDecimals(base) - quoteDecimals),
    entryVps: quoteUnits(header.entryVps) / Q32,
    openedAtSlot: Number(header.openedAt),
    realizedPnl: quoteUnits(header.realizedPnl),
    realizedYield: quoteUnits(header.realizedYield),
    realizedFx: quoteUnits(header.realizedFx),
  };
}

// Every roster market that exists on this cluster, with its live vaults.
// Markets resolve in parallel; a pair with no market is simply absent. The
// route already carries the market's raw bytes, so each market costs its
// resolve and nothing more.
export async function fetchChainVaultMarkets(rpc: Rpc): Promise<VaultMarket[]> {
  await initSimulator();
  const markets = await Promise.all(
    VAULT_PAIR_ROSTER.map(async ({ base: a, quote: b }) => {
      const route = await resolveEclobRoute(
        rpc,
        stablecoinMint(a),
        stablecoinMint(b),
      );
      if (!route) return null;
      // sell ⇒ the market's base is the first mint passed (see useOrderBook).
      const base = route.side === "sell" ? a : b;
      const quote = route.side === "sell" ? b : a;
      const vaults = decodeDropsetVaults(route.marketData).map((s) =>
        chainVault(s, route.market, base, quote),
      );
      return resolveVaultMarket(route.market, base, quote, 0, vaults);
    }),
  );
  return markets.filter((m): m is VaultMarket => m !== null);
}

// The owner's positions across the live vaults, keyed by chainVaultId. One
// batched account read: the PDAs are derived locally, and a closed or never
// opened position is just a missing account.
export async function fetchChainPositions(
  rpc: Rpc,
  owner: string,
  markets: VaultMarket[],
): Promise<Map<string, VaultPosition>> {
  const signer = address(owner);
  const targets = await Promise.all(
    markets.flatMap((m) =>
      m.vaults.flatMap((v) => {
        const chain = v.chain;
        if (!chain) return [];
        return [
          findVaultDepositorPda({
            market: address(m.marketPubkey),
            vaultIdx: chain.index,
            signer,
          }).then(([pda]) => ({
            pda,
            vaultId: v.vaultPubkey,
            base: m.base,
            quote: m.quote,
          })),
        ];
      }),
    ),
  );
  const out = new Map<string, VaultPosition>();
  if (targets.length === 0) return out;
  const accounts = await fetchAllMaybeVaultDepositorHeader(
    rpc,
    targets.map((t) => t.pda),
    { commitment: "confirmed" },
  );
  accounts.forEach((account, i) => {
    const t = targets[i];
    if (!t || !account.exists || account.data.shares === 0n) return;
    out.set(t.vaultId, chainPosition(account.data, t.vaultId, t.base, t.quote));
  });
  return out;
}
