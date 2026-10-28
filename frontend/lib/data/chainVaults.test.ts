import {
  encodePrice,
  PRICE_ZERO,
  type VaultDepositorHeader,
  type VaultSummary,
} from "@dropset/sdk";
import { address } from "@solana/kit";
import { describe, expect, it } from "vitest";
import { VAULT_SEQ_PARAM, vaultLinkHref } from "../ui/swapUrl";
import {
  chainPosition,
  chainVault,
  chainVaultId,
  marketOrientation,
} from "./chainVaults";
import { positionPnl } from "./pnl";
import {
  contributeClosedReason,
  depositClosedReason,
  findVaultBySeq,
  resolveVaultMarket,
  tokenUsdPrice,
  vaultLabel,
  vaultRefPrice,
} from "./vaults";

const MARKET = "11111111111111111111111111111111";
const LEADER = address("So11111111111111111111111111111111111111112");

// TGBP carries 9 decimals and USDC 6, so this pair exercises the decimals
// gap the entry price has to be rescaled across.
const summary = (over: Partial<VaultSummary> = {}): VaultSummary => ({
  index: 2,
  seq: 5n,
  name: null,
  leader: LEADER,
  // 1.25 USDC per TGBP, as a quote-atoms-per-base-atom ratio.
  referencePrice: encodePrice(1.25e-3),
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
    // The program's index: isqrt(base_atoms · quote_atoms) / total_shares.
    expect(v.vps).toBeCloseTo(Math.sqrt(1e12 * 1.27e9) / 2_000_000, 6);
  });

  it("carries the on-chain name, unset staying null", () => {
    expect(chainVault(summary(), MARKET, "TGBP", "USDC").name).toBeNull();
    expect(
      chainVault(summary({ name: "GBP Basis" }), MARKET, "TGBP", "USDC").name,
    ).toBe("GBP Basis");
  });

  it("labels a vault by its name, else #seq", () => {
    const named = chainVault(
      summary({ name: "GBP Basis" }),
      MARKET,
      "TGBP",
      "USDC",
    );
    expect(vaultLabel(named)).toBe("GBP Basis");
    // The per-market seq (5), not the sector index (2).
    const unnamed = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(vaultLabel(unnamed)).toBe("#5");
  });

  it("values TVL at the vault's own reference price", () => {
    // USD-quoted: 1,000 TGBP × 1.25 + 1,270 USDC, no placeholder price.
    const v = chainVault(summary(), MARKET, "TGBP", "USDC");
    expect(v.refPrice).toBeCloseTo(1.25, 6);
    expect(v.quoteUsd).toBe(1);
    expect(v.usdIndicative).toBe(false);
    expect(v.tvl).toBeCloseTo(2_520, 4);
    expect(vaultRefPrice(v)).toBeCloseTo(1.25, 6);

    // USD-based (USD/JPY quotes GYEN, both 6 decimals): 150 GYEN per USDC
    // makes a GYEN worth 1/150 USD, so the whole vault is valued in base.
    const jpy = chainVault(
      summary({
        referencePrice: encodePrice(150),
        baseAtoms: 1_000n * 10n ** 6n,
        quoteAtoms: 150_000n * 10n ** 6n,
      }),
      MARKET,
      "USDC",
      "GYEN",
    );
    expect(jpy.quoteUsd).toBeCloseTo(1 / 150, 9);
    expect(jpy.usdIndicative).toBe(false);
    expect(jpy.tvl).toBeCloseTo(2_000, 4);
  });

  it("falls back to indicative prices only while the book is dark", () => {
    const dark = chainVault(
      summary({ referencePrice: PRICE_ZERO }),
      MARKET,
      "TGBP",
      "USDC",
    );
    expect(dark.refPrice).toBeNull();
    expect(dark.usdIndicative).toBe(true);
    expect(dark.tvl).toBeCloseTo(1_000 * tokenUsdPrice("TGBP") + 1_270, 4);
    // PnL then marks at the reserve ratio, as before a price was decoded.
    expect(vaultRefPrice(dark)).toBeCloseTo(1.27, 6);

    // A dark USD-based market has no on-chain quote→USD rate either, so the
    // quote leg falls back to the table too.
    const darkJpy = chainVault(
      summary({
        referencePrice: PRICE_ZERO,
        baseAtoms: 1_000n * 10n ** 6n,
        quoteAtoms: 150_000n * 10n ** 6n,
      }),
      MARKET,
      "USDC",
      "GYEN",
    );
    expect(darkJpy.refPrice).toBeNull();
    expect(darkJpy.quoteUsd).toBe(tokenUsdPrice("GYEN"));
    expect(darkJpy.usdIndicative).toBe(true);
    expect(darkJpy.tvl).toBeCloseTo(1_000 + 150_000 * tokenUsdPrice("GYEN"), 4);
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
        realizedPrice: 0n,
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

describe("contributeClosedReason", () => {
  const PAIR = { base: "TGBP", quote: "USDC" };
  const vault = (over: Partial<VaultSummary> = {}) =>
    chainVault(summary(over), MARKET, PAIR.base, PAIR.quote);

  it("is open on an open, two-sided vault with room under the floor", () => {
    expect(contributeClosedReason(PAIR, vault())).toBeNull();
  });

  it("says not open yet when the opt-in or the approval is missing", () => {
    for (const over of [
      { totalShares: 0n },
      { allowOutsideDepositors: false },
      { outsideDepositsApproved: false },
    ]) {
      expect(contributeClosedReason(PAIR, vault(over))).toMatch(
        /^Not open yet/,
      );
    }
  });

  it("names the leg a one-sided vault still holds", () => {
    expect(contributeClosedReason(PAIR, vault({ quoteAtoms: 0n }))).toMatch(
      /holds only TGBP.*rebalances/,
    );
    expect(contributeClosedReason(PAIR, vault({ baseAtoms: 0n }))).toMatch(
      /holds only USDC/,
    );
  });

  it("closes at the leader floor with the outside-capital multiple", () => {
    // A 5% floor caps the vault at 20x the leader's stake: 19x outside.
    const full = vault({
      leaderShares: 100_000n,
      totalShares: 2_000_000n,
      minLeaderSharePpm: 50_000,
    });
    expect(contributeClosedReason(PAIR, full)).toMatch(
      /cap of 19 times the leader's stake.*5%/,
    );
    expect(
      contributeClosedReason(
        PAIR,
        vault({ leaderShares: 100_001n, minLeaderSharePpm: 50_000 }),
      ),
    ).toBeNull();
  });

  it("decides the floor in integers, exactly at one share of room", () => {
    // 0.9% of 1,000 shares is exactly the leader's 9: no share fits. The
    // float cap (9 / 0.009 lands just above 1,000) reads a sliver of room.
    const atFloor = { leaderShares: 9n, minLeaderSharePpm: 9_000 };
    expect(
      contributeClosedReason(PAIR, vault({ ...atFloor, totalShares: 1_000n })),
    ).toMatch(/^Full/);
    expect(
      contributeClosedReason(PAIR, vault({ ...atFloor, totalShares: 999n })),
    ).toBeNull();
    // No floor sets no cap; a floor with no leader stake left fits nothing.
    expect(
      contributeClosedReason(PAIR, vault({ minLeaderSharePpm: 0 })),
    ).toBeNull();
    expect(contributeClosedReason(PAIR, vault({ leaderShares: 0n }))).toMatch(
      /^Full/,
    );
  });

  it("names an empty vault, and puts the program's gates first", () => {
    expect(
      contributeClosedReason(PAIR, vault({ baseAtoms: 0n, quoteAtoms: 0n })),
    ).toMatch(/holds neither asset/);
    expect(
      contributeClosedReason(PAIR, vault({ frozen: true, quoteAtoms: 0n })),
    ).toMatch(/frozen/);
    expect(contributeClosedReason(PAIR, vault(), LEADER)).toMatch(/lead/);
  });
});

describe("the public vault link", () => {
  const live = chainVault(summary(), MARKET, "TGBP", "USDC");
  const markets = [
    resolveVaultMarket(MARKET, "TGBP", "USDC", [
      chainVault(summary({ seq: 4n }), MARKET, "TGBP", "USDC"),
      live,
    ]),
  ];

  it("round-trips the pair and seq through the URL", () => {
    const href = vaultLinkHref("TGBP", "USDC", 5n);
    const params = new URL(href, "https://dropset.example").searchParams;
    expect(params.get("base")).toBe("TGBP");
    expect(params.get(VAULT_SEQ_PARAM)).toBe("5");
    const hit = findVaultBySeq(
      markets,
      params.get("base") ?? "",
      params.get("quote") ?? "",
      params.get(VAULT_SEQ_PARAM) ?? "",
    );
    expect(hit?.vault).toBe(live);
  });

  it("matches nothing for another pair, an unknown seq, or a non-integer", () => {
    expect(findVaultBySeq(markets, "USDC", "TGBP", "5")).toBeNull();
    expect(findVaultBySeq(markets, "TGBP", "USDC", "6")).toBeNull();
    expect(findVaultBySeq(markets, "TGBP", "USDC", "5x")).toBeNull();
    expect(findVaultBySeq(markets, "TGBP", "USDC", "")).toBeNull();
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
      realizedPrice: -4n * 10n ** 6n,
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
    expect(p.realizedPrice).toBe(-4);
  });
});
