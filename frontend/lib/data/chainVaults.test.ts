import {
  encodePrice,
  type VaultDepositorHeader,
  type VaultSummary,
} from "@dropset/sdk";
import { address } from "@solana/kit";
import { describe, expect, it } from "vitest";
import {
  chainPosition,
  chainVault,
  chainVaultId,
  marketOrientation,
} from "./chainVaults";
import { positionPnl } from "./pnl";
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
  hwm: 7n << 32n,
  perfFeeRatePpm: 200_000,
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
    expect(v.chain).toEqual({
      index: 2,
      seq: 5n,
      baseAtoms: 1_000n * 10n ** 9n,
      quoteAtoms: 1_270n * 10n ** 6n,
      totalShares: 2_000_000n,
      leaderShares: 1_500_000n,
      hwm: 7n << 32n,
      perfFeeRatePpm: 200_000,
      frozen: false,
    });
    expect(v.vaultPubkey).toBe(chainVaultId(MARKET, 5n));
    expect(v.volume24h).toBeNull();
    // The program's index: isqrt(base_atoms · quote_atoms) / total_shares.
    expect(v.vps).toBeCloseTo(Math.sqrt(1e12 * 1.27e9) / 2_000_000, 6);
  });

  it("compares a position's entry VPS like with like", () => {
    // A depositor who entered at exactly this vault state has earned nothing,
    // so the program's own entry VPS for that state must read back as 0%.
    const s = summary();
    const lAtoms = BigInt(Math.floor(Math.sqrt(1e12 * 1.27e9)));
    const entryVps = (lAtoms << 32n) / s.totalShares;
    const v = chainVault(s, MARKET, "TGBP", "USDC");
    const p = chainPosition(
      {
        shares: 1n,
        netDeposits: 0n,
        grossDeposited: 0n,
        entryRefPrice: encodePrice(1.27e-3),
        entryVps,
        openedAt: 0n,
        realizedPnl: 0n,
        realizedYield: 0n,
        realizedFx: 0n,
      } as VaultDepositorHeader,
      v.vaultPubkey,
      "TGBP",
      "USDC",
    );
    expect(positionPnl(p, v, 1.27).yieldPctSinceOpen).toBeCloseTo(0, 6);

    // A vault holding one leg has an index of 0, which measures nothing:
    // the figure is undefined, not a −100% loss.
    const oneSided = chainVault(
      summary({ quoteAtoms: 0n }),
      MARKET,
      "TGBP",
      "USDC",
    );
    expect(oneSided.vps).toBe(0);
    expect(positionPnl(p, oneSided, 1.27).yieldPctSinceOpen).toBeNull();
  });

  it("orients each market by the take side its route resolved", () => {
    expect(marketOrientation("sell", "TGBP", "USDC")).toEqual({
      base: "TGBP",
      quote: "USDC",
    });
    expect(marketOrientation("buy", "USDC", "TGBP")).toEqual({
      base: "TGBP",
      quote: "USDC",
    });
  });

  it("closes deposits on each of the program's flag gates", () => {
    const open = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(depositClosedReason(open)).toBeNull();
    for (const over of [
      { frozen: true },
      { totalShares: 0n },
      { allowOutsideDepositors: false },
      { outsideDepositsApproved: false },
    ]) {
      const v = chainVault(summary(over), MARKET, "TGBP", "USDC");
      expect(depositClosedReason(v)).not.toBeNull();
    }
  });

  it("closes deposits to the vault's own leader only", () => {
    const open = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(depositClosedReason(open, LEADER)).not.toBeNull();
    expect(depositClosedReason(open, MARKET)).toBeNull();
    // A raw decoded summary gates the same way as the table row.
    expect(depositClosedReason(summary({ frozen: true }))).not.toBeNull();
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
      // Q32.32 of the program's raw-atoms index; kept raw, not rescaled.
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
    expect(p.entryVps).toBe(1_270);
    expect(p.openedAtSlot).toBe(42);
    expect(p.realizedPnl).toBe(-3);
    expect(p.realizedYield).toBe(1);
    expect(p.realizedFx).toBe(-4);
  });
});
