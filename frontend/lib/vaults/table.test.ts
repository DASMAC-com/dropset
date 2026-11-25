import { encodePrice, type VaultSummary } from "@dropset/sdk";
import { address } from "@solana/kit";
import { describe, expect, it } from "vitest";
import { chainVault } from "@/lib/data/chainVaults";
import {
  type FxPairGroup,
  type GroupedVault,
  groupVaultMarkets,
  resolveVaultMarket,
  type Vault,
  type VaultPin,
} from "@/lib/data/vaults";
import {
  arrangeSection,
  arrangeVaultsTable,
  groupSortValue,
  matchesQuery,
  type PositionValue,
  vaultSortValue,
} from "./table";

const GBP_MARKET = "11111111111111111111111111111111";
const CAD_MARKET = "SysvarC1ock11111111111111111111111111111111";
const LEADER = address("So11111111111111111111111111111111111111112");
const OTHER = address("Vote111111111111111111111111111111111111111");

const summary = (over: Partial<VaultSummary>): VaultSummary => ({
  index: 0,
  seq: 1n,
  name: null,
  leader: LEADER,
  referencePrice: encodePrice(1e-3),
  baseAtoms: 0n,
  quoteAtoms: 0n,
  totalShares: 1n,
  leaderShares: 1n,
  hwm: 1n << 32n,
  perfFeeRatePpm: 0,
  minLeaderSharePpm: 0,
  frozen: false,
  allowOutsideDepositors: true,
  outsideDepositsApproved: true,
  ...over,
});

// A live vault with its TVL set outright, so the order under test reads off
// the fixture rather than the reserve valuation.
const vault = (
  market: string,
  base: string,
  over: Partial<VaultSummary>,
  tvl: number,
): Vault => ({ ...chainVault(summary(over), market, base, "USDC"), tvl });

const alpha = vault(GBP_MARKET, "TGBP", { seq: 1n, name: "Alpha" }, 100);
const second = vault(GBP_MARKET, "TGBP", { seq: 2n }, 300);
const cad = vault(CAD_MARKET, "CADC", { seq: 1n, leader: OTHER }, 50);

// GBP / USD listed first, so the default order's alphabetical pairs are not
// just the input order.
const groups = groupVaultMarkets([
  resolveVaultMarket(GBP_MARKET, "TGBP", "USDC", [alpha, second]),
  resolveVaultMarket(CAD_MARKET, "CADC", "USDC", [cad]),
]);

const NO_PIN: VaultPin = { base: "", quote: "", vault: "", leader: "" };
const noPosition: PositionValue = () => 0;

const vaultsOf = (rows: GroupedVault[]) => rows.map((gv) => gv.vault);
const labelsOf = (section: { groups: { group: FxPairGroup }[] }) =>
  section.groups.map(({ group }) => group.label);

const group = (label: string): FxPairGroup => {
  const g = groups.find((x) => x.label === label);
  if (!g) throw new Error(`fixture group ${label} missing`);
  return g;
};
const entry = (v: Vault): GroupedVault => {
  const hit = groups.flatMap((g) => g.vaults).find((gv) => gv.vault === v);
  if (!hit) throw new Error("fixture vault missing");
  return hit;
};

const arrange = (
  state: Partial<Parameters<typeof arrangeVaultsTable>[1]> = {},
) =>
  arrangeVaultsTable(groups, {
    query: "",
    pin: NO_PIN,
    sort: null,
    positionValue: noPosition,
    ...state,
  });

describe("matchesQuery", () => {
  it("matches everything on an empty query", () => {
    expect(matchesQuery("", group("CAD / USD"), entry(cad))).toBe(true);
  });

  it("matches the pair, the tokens, the name, the number and the leader", () => {
    const gbp = group("GBP / USD");
    for (const q of ["gbp / usd", "tgbp", "usdc", "alpha", "vault #1"]) {
      expect(matchesQuery(q, gbp, entry(alpha)), q).toBe(true);
    }
    expect(matchesQuery(LEADER.toLowerCase(), gbp, entry(alpha))).toBe(true);
    expect(matchesQuery("alpha", gbp, entry(second))).toBe(false);
    expect(matchesQuery("cadc", gbp, entry(alpha))).toBe(false);
  });
});

describe("the sort values", () => {
  const position: PositionValue = (v) => (v === alpha ? 7 : 0);

  it("reads a vault's position, label, pair and TVL", () => {
    expect(vaultSortValue(entry(alpha), "position", position)).toBe(7);
    expect(vaultSortValue(entry(alpha), "name", position)).toBe("Alpha");
    expect(vaultSortValue(entry(second), "name", position)).toBe("Vault #2");
    expect(vaultSortValue(entry(alpha), "pair", position)).toBe("TGBP / USDC");
    expect(vaultSortValue(entry(alpha), "tvl", position)).toBe(100);
  });

  it("sums a pair's positions and ranks it by its first label", () => {
    const gbp = group("GBP / USD");
    expect(groupSortValue(gbp, "position", position)).toBe(7);
    expect(groupSortValue(gbp, "name", position)).toBe("Alpha");
    expect(groupSortValue(gbp, "pair", position)).toBe("GBP / USD");
    expect(groupSortValue(gbp, "tvl", position)).toBe(400);
  });
});

describe("arrangeSection", () => {
  it("drops a group its filter empties, in both views", () => {
    const section = arrangeSection(
      groups,
      (_, e) => e.vault !== cad,
      () => 0,
      () => 0,
    );
    expect(labelsOf(section)).toEqual(["GBP / USD"]);
    expect(vaultsOf(section.flat)).toEqual([alpha, second]);
  });
});

describe("arrangeVaultsTable", () => {
  it("orders pairs by base code and each pair's vaults by TVL by default", () => {
    const section = arrange();
    expect(labelsOf(section)).toEqual(["CAD / USD", "GBP / USD"]);
    expect(section.groups.map((g) => vaultsOf(g.vaults))).toEqual([
      [cad],
      [second, alpha],
    ]);
    expect(vaultsOf(section.flat)).toEqual([cad, second, alpha]);
  });

  it("filters on the trimmed, case-folded query", () => {
    const section = arrange({ query: "  ALPHA " });
    expect(labelsOf(section)).toEqual(["GBP / USD"]);
    expect(vaultsOf(section.flat)).toEqual([alpha]);
  });

  it("filters on the URL pin", () => {
    const section = arrange({ pin: { ...NO_PIN, leader: OTHER } });
    expect(vaultsOf(section.flat)).toEqual([cad]);
  });

  it("sorts groups and rows by the chosen column", () => {
    const section = arrange({ sort: { key: "tvl", direction: "asc" } });
    expect(labelsOf(section)).toEqual(["CAD / USD", "GBP / USD"]);
    expect(vaultsOf(section.flat)).toEqual([cad, alpha, second]);
    const desc = arrange({ sort: { key: "tvl", direction: "desc" } });
    expect(labelsOf(desc)).toEqual(["GBP / USD", "CAD / USD"]);
    expect(vaultsOf(desc.flat)).toEqual([second, alpha, cad]);
  });

  it("sorts by the supplied position value", () => {
    const section = arrange({
      sort: { key: "position", direction: "desc" },
      positionValue: (v) => (v === alpha ? 7 : 0),
    });
    expect(labelsOf(section)[0]).toBe("GBP / USD");
    expect(vaultsOf(section.flat)[0]).toBe(alpha);
  });
});
