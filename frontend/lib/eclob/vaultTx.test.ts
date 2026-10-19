import { BasketError } from "@dropset/sdk";
import { describe, expect, it } from "vitest";
import { applySlippage } from "./eclobSwap";
import {
  depositSizingError,
  leaderFloorHolds,
  sharesForFraction,
  sizeDeposit,
  sizeWithdraw,
  type VaultAtoms,
} from "./vaultTx";

// 1,000 TGBP (9 decimals) against 1,270 USDC (6 decimals) over 2M shares.
const VAULT: VaultAtoms = {
  baseAtoms: 1_000n * 10n ** 9n,
  quoteAtoms: 1_270n * 10n ** 6n,
  totalShares: 2_000_000n,
};
const SYMBOLS = { base: "TGBP", quote: "USDC" };

describe("sizeDeposit", () => {
  it("sizes the base leg as the program does and widens only the derived leg", () => {
    // 10 TGBP is 1% of the base reserve: 20,000 shares, 12.7 USDC pulled.
    const s = sizeDeposit(VAULT, "base", 10n * 10n ** 9n, 50);
    expect(s.sharesOut).toBe(20_000n);
    expect(s.baseIn).toBe(10n * 10n ** 9n);
    expect(s.quoteIn).toBe(12_700_000n);
    expect(s.maxBaseIn).toBe(10n * 10n ** 9n);
    // 12.7 USDC plus 0.5%, rounded up.
    expect(s.maxQuoteIn).toBe(12_763_500n);
  });

  it("rounds the widened bound up when the tolerance isn't a whole atom", () => {
    // A derived final of 12,700,001 at 50 bps widens by 63,500.005 atoms: the
    // bound takes the ceiling, so it never sits a fractional atom short.
    const v = { ...VAULT, quoteAtoms: 1_270_000_001n };
    const s = sizeDeposit(v, "base", 10n * 10n ** 9n, 50);
    expect(s.quoteIn).toBe(12_700_001n);
    expect(s.maxQuoteIn).toBe(12_763_502n);
  });

  it("sizes the quote leg symmetrically", () => {
    const s = sizeDeposit(VAULT, "quote", 12_700_000n, 50);
    expect(s.sharesOut).toBe(20_000n);
    expect(s.baseIn).toBe(10n * 10n ** 9n);
    expect(s.maxQuoteIn).toBe(12_700_000n);
    expect(s.maxBaseIn).toBe(10n * 10n ** 9n + 50_000_000n);
  });

  it("never pulls more of the driven leg than was entered", () => {
    // Shares floor and the basket rounds up, so the driven leg's final could
    // only exceed the entry if the rounding went the wrong way.
    for (const amount of [1_234_567_891n, 999_999_999n, 7n * 10n ** 9n + 3n]) {
      const s = sizeDeposit(VAULT, "base", amount, 0);
      expect(s.baseIn).toBeLessThanOrEqual(amount);
      expect(s.maxBaseIn).toBe(amount);
    }
  });

  it("rejects what the program rejects, with a reason naming the legs", () => {
    const oneSided = { ...VAULT, quoteAtoms: 0n };
    let caught: unknown;
    try {
      sizeDeposit(oneSided, "quote", 1_000_000n, 50);
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(BasketError);
    expect(depositSizingError(caught, "quote", SYMBOLS)).toMatch(
      /holds no USDC.*TGBP/,
    );

    // One base atom buys no whole share.
    expect(() => sizeDeposit(VAULT, "base", 1n, 50)).toThrow(BasketError);
    try {
      sizeDeposit(VAULT, "base", 1n, 50);
    } catch (e) {
      expect(depositSizingError(e, "base", SYMBOLS)).toMatch(/single/);
    }
  });
});

describe("sharesForFraction", () => {
  it("redeems every share at 100%, so a full withdraw closes the position", () => {
    expect(sharesForFraction(3n, 10_000)).toBe(3n);
    expect(sharesForFraction(3n, 12_000)).toBe(3n);
  });

  it("floors a partial fraction and clamps below zero", () => {
    expect(sharesForFraction(3n, 5_000)).toBe(1n);
    expect(sharesForFraction(1_000n, 2_500)).toBe(250n);
    expect(sharesForFraction(1_000n, -5)).toBe(0n);
    // A non-finite fraction redeems nothing rather than throwing.
    expect(sharesForFraction(1_000n, Number.NaN)).toBe(0n);
  });
});

describe("sizeWithdraw", () => {
  it("slices pro rata and floors each leg by the tolerance", () => {
    const s = sizeWithdraw(VAULT, 20_000n, 50);
    expect(s.baseOut).toBe(10n * 10n ** 9n);
    expect(s.quoteOut).toBe(12_700_000n);
    expect(s.minBaseOut).toBe(applySlippage(s.baseOut, 50));
    expect(s.minQuoteOut).toBe(applySlippage(s.quoteOut, 50));
    expect(s.minQuoteOut).toBeLessThan(s.quoteOut);
  });
});

describe("leaderFloorHolds", () => {
  it("applies the program's integer floor at its exact boundary", () => {
    // A 10% floor on 1.5M leader shares caps the total at 15M shares.
    const v = {
      leaderShares: 1_500_000n,
      totalShares: 2_000_000n,
      minLeaderSharePpm: 100_000,
    };
    expect(leaderFloorHolds(v, 13_000_000n)).toBe(true);
    expect(leaderFloorHolds(v, 13_000_001n)).toBe(false);
  });
});
