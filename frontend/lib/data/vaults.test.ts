import { describe, expect, it } from "vitest";
import {
  ALL_STABLECOINS,
  currencyForStablecoin,
  stablecoinMint,
} from "./currencies";
import { findVaultPair, VAULT_PAIR_ROSTER } from "./vaults";

describe("the vault pair roster", () => {
  const symbols = VAULT_PAIR_ROSTER.flatMap((p) => [p.base, p.quote]);

  // A market outside the roster is never probed, so its vaults never show:
  // every non-USD stablecoin must have its USDC pair, not only the majors.
  it("pairs every non-USD stablecoin against USDC", () => {
    const nonUsd = ALL_STABLECOINS.filter(
      (s) => currencyForStablecoin(s.symbol) !== "USD",
    ).map((s) => s.symbol);
    expect(nonUsd).toContain("ZARP");
    for (const symbol of nonUsd) {
      expect(findVaultPair(symbol, "USDC"), symbol).not.toBeNull();
    }
    expect(VAULT_PAIR_ROSTER).toHaveLength(nonUsd.length);
  });

  it("names only symbols with a mint and a currency", () => {
    for (const symbol of symbols) {
      expect(stablecoinMint(symbol), symbol).not.toBe("");
      expect(currencyForStablecoin(symbol), symbol).toBeDefined();
    }
  });

  it("lists each unordered pair once", () => {
    const keys = VAULT_PAIR_ROSTER.map((p) =>
      [p.base, p.quote].sort().join("/"),
    );
    expect(new Set(keys).size).toBe(keys.length);
  });
});

describe("findVaultPair", () => {
  it("finds a pair whichever way it is asked", () => {
    const want = { base: "EURC", quote: "USDC" };
    expect(findVaultPair("EURC", "USDC")).toEqual(want);
    expect(findVaultPair("USDC", "EURC")).toEqual(want);
  });

  it("answers null for a pair the roster lacks", () => {
    expect(findVaultPair("EURC", "GYEN")).toBeNull();
  });
});
