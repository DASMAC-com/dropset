import {
  encodePrice,
  type VaultDepositorHeader,
  type VaultSummary,
} from "@dropset/sdk";
import { address } from "@solana/kit";
import { describe, expect, it } from "vitest";
import { chainPosition, chainVault, chainVaultId } from "./chainVaults";
import { depositClosedReason } from "./vaults";

const MARKET = "11111111111111111111111111111111";
const LEADER = address("So11111111111111111111111111111111111111112");

// TGBP carries 9 decimals and USDC 6, so this pair exercises the decimals
// gap the entry price has to be rescaled across.
const summary = (over: Partial<VaultSummary> = {}): VaultSummary => ({
  index: 2,
  seq: 5n,
  leader: LEADER,
  baseAtoms: 1_000n * 10n ** 9n, // 1,000 TGBP
  quoteAtoms: 1_270n * 10n ** 6n, // 1,270 USDC
  totalShares: 2_000_000n,
  leaderShares: 1_500_000n,
  minLeaderSharePpm: 100_000,
  frozen: false,
  allowOutsideDepositors: true,
  outsideDepositsApproved: true,
  ...over,
});

describe("chainVault", () => {
  it("scales reserves to whole tokens and keeps shares raw", () => {
    const v = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(v.baseReserve).toBe(1_000);
    expect(v.quoteReserve).toBe(1_270);
    expect(v.totalShares).toBe(2_000_000);
    expect(v.chain).toEqual({ index: 2, seq: 5n });
    expect(v.vaultPubkey).toBe(chainVaultId(MARKET, 5n));
    expect(v.volume24h).toBeNull();
    // Value per share at the reserve ratio: (1000 · 1.27 + 1270) / 2e6.
    expect(v.vps).toBeCloseTo(2_540 / 2_000_000, 12);
  });

  it("closes deposits on each of the program's three gates", () => {
    const open = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(depositClosedReason(open)).toBeNull();
    for (const over of [
      { frozen: true },
      { allowOutsideDepositors: false },
      { outsideDepositsApproved: false },
    ]) {
      const v = chainVault(summary(over), MARKET, "TGBP", "USDC");
      expect(depositClosedReason(v)).not.toBeNull();
    }
  });
});

describe("chainPosition", () => {
  it("converts quote atoms, the entry price and the Q32.32 entry VPS", () => {
    const header = {
      market: address(MARKET),
      sectorIdx: 2,
      owner: LEADER,
      shares: 100_000n,
      netDeposits: 127n * 10n ** 6n,
      grossDeposited: 200n * 10n ** 6n,
      // 1.27 USDC per TGBP, as a quote-atoms-per-base-atom ratio.
      entryRefPrice: encodePrice(1.27e-3),
      // 0.00127 USDC (1,270 quote atoms) per share, Q32.32.
      entryVps: 1_270n << 32n,
      openedAt: 42n,
      realizedPnl: -3n * 10n ** 6n,
      realizedYield: 1n * 10n ** 6n,
      realizedFx: -4n * 10n ** 6n,
    } as VaultDepositorHeader;
    const p = chainPosition(header, "id", "TGBP", "USDC");
    expect(p.shares).toBe(100_000);
    expect(p.netDeposits).toBe(127);
    expect(p.grossDeposited).toBe(200);
    expect(p.entryRefPrice).toBeCloseTo(1.27, 4);
    expect(p.entryVps).toBeCloseTo(0.00127, 9);
    expect(p.realizedPnl).toBe(-3);
    expect(p.realizedFx).toBe(-4);
  });
});
