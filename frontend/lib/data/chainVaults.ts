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
import { type Address, address } from "@solana/kit";
import { resolveEclobRoute } from "../eclob/route";
import { GET_MULTIPLE_ACCOUNTS_BATCH_SIZE } from "../env";
import { stablecoinDecimals, stablecoinMint } from "./currencies";
import type { VaultPosition } from "./positions";
import {
  resolveVaultMarket,
  tokenUsdPrice,
  VAULT_PAIR_ROSTER,
  type Vault,
  type VaultMarket,
} from "./vaults";

// The live half of the vaults page: every vault on every roster market, read
// straight off the market accounts (one account per market, no indexer), and
// the connected wallet's positions read off its VaultDepositor PDAs.
//
// The UI's Vault / VaultPosition shapes are whole-token floats, so this is
// where atoms meet display units. Shares stay raw counts: every consumer uses
// them only as a ratio to totalShares, and both sides of that ratio are raw.

type Rpc = SolanaClientRuntime["rpc"];

// The entry VPS is Q32.32 on-chain.
const Q32 = 2 ** 32;

const atomsToUnits = (atoms: bigint, decimals: number): number =>
  Number(atoms) / 10 ** decimals;

// A live vault's id. There is no vault account — a vault is a sector of its
// market — so the row key and the position lookup use (market, seq), the
// pair `seq` is documented to identify.
export function chainVaultId(market: string, seq: bigint): string {
  return `${market}:${seq}`;
}

// The value-per-share index the program stamps as a depositor's entry VPS:
// `L / total_shares` with `L = isqrt(base_atoms · quote_atoms)` (the deposit
// handler), so it is in raw atoms and shares, not in quote. Computed here the
// same way so `vault.vps / position.entryVps` compares like with like; the
// units cancel in that ratio, which is all a consumer reads.
const chainVps = (summary: VaultSummary): number =>
  summary.totalShares > 0n
    ? Math.sqrt(Number(summary.baseAtoms) * Number(summary.quoteAtoms)) /
      Number(summary.totalShares)
    : 0;

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
  return {
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
    vps: chainVps(summary),
    minLeaderSharePpm: summary.minLeaderSharePpm,
    frozen: summary.frozen,
    allowOutsideDepositors: summary.allowOutsideDepositors,
    outsideDepositsApproved: summary.outsideDepositsApproved,
    name: null,
  };
}

// A VaultDepositor account in the UI's position shape. Quote-denominated
// figures are quote atoms on-chain; the entry reference price is an
// atoms ratio, rescaled to whole tokens by the decimals gap; the entry VPS
// stays in the raw units `chainVps` uses.
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
    entryVps: Number(header.entryVps) / Q32,
    openedAtSlot: Number(header.openedAt),
    realizedPnl: quoteUnits(header.realizedPnl),
    realizedYield: quoteUnits(header.realizedYield),
    realizedFx: quoteUnits(header.realizedFx),
  };
}

// The market's own base/quote for a roster pair `(a, b)`, from the take side
// the route resolved for an a→b trade: selling `a` means `a` is the base
// (see useOrderBook).
export function marketOrientation(
  side: "buy" | "sell",
  a: string,
  b: string,
): { base: string; quote: string } {
  return side === "sell" ? { base: a, quote: b } : { base: b, quote: a };
}

// Every roster market that exists on this cluster, with its live vaults.
// Markets resolve in parallel; a pair with no market is simply absent, and so
// is one whose read fails this round, so one bad market never blanks the
// rest. The route already carries the market's raw bytes, so each market
// costs its resolve and nothing more. Throws only when every market fails.
export async function fetchChainVaultMarkets(rpc: Rpc): Promise<VaultMarket[]> {
  await initSimulator();
  const results = await Promise.allSettled(
    VAULT_PAIR_ROSTER.map(async ({ base: a, quote: b }) => {
      const route = await resolveEclobRoute(
        rpc,
        stablecoinMint(a),
        stablecoinMint(b),
      );
      if (!route) return null;
      const { base, quote } = marketOrientation(route.side, a, b);
      const vaults = decodeDropsetVaults(route.marketData).map((s) =>
        chainVault(s, route.market, base, quote),
      );
      return resolveVaultMarket(route.market, base, quote, 0, vaults);
    }),
  );
  if (results.every((r) => r.status === "rejected")) {
    throw new AggregateError(
      results.map((r) => (r.status === "rejected" ? r.reason : null)),
      "every vault market read failed",
    );
  }
  return results.flatMap((r) =>
    r.status === "fulfilled" && r.value ? [r.value] : [],
  );
}

// The owner's positions across the live vaults, keyed by chainVaultId. The
// PDAs are derived locally and read in batches of the provider's
// getMultipleAccounts cap (as useAllBalances does); a closed or never opened
// position is just a missing account.
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
  const size = GET_MULTIPLE_ACCOUNTS_BATCH_SIZE || targets.length;
  const chunks: Address[][] = [];
  for (let i = 0; i < targets.length; i += size) {
    chunks.push(targets.slice(i, i + size).map((t) => t.pda));
  }
  const accounts = (
    await Promise.all(
      chunks.map((chunk) =>
        fetchAllMaybeVaultDepositorHeader(rpc, chunk, {
          commitment: "confirmed",
        }),
      ),
    )
  ).flat();
  accounts.forEach((account, i) => {
    const t = targets[i];
    if (!t || !account.exists || account.data.shares === 0n) return;
    out.set(t.vaultId, chainPosition(account.data, t.vaultId, t.base, t.quote));
  });
  return out;
}
