import { describe, expect, it } from "vitest";
import { currencyForStablecoin, stablecoinMint } from "./currencies";
import { findVaultPair, VAULT_PAIR_ROSTER } from "./vaults";

// The roster is a literal, so nothing resolves its symbols at load: a typo
// would only drop that pair from the live read (stablecoinMint answers "").
// These checks stand in for that missing load-time resolve.
describe("the vault pair roster", () => {
  const symbols = VAULT_PAIR_ROSTER.flatMap((p) => [p.base, p.quote]);

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
