import { describe, expect, it } from "vitest";
import type { Vault } from "@/lib/data/vaults";
import {
  type DepositFormInput,
  depositForm,
  fillOtherLeg,
  sizeLeg,
} from "./depositForm";

// EURC and USDC both carry six decimals.
const MARKET = { base: "EURC", quote: "USDC" };
const ATOMS = 1_000_000n;
const OWNER = "Owner1111111111111111111111111111111111111";

// A two-sided vault holding 1,000 EURC and 1,100 USDC over a million shares,
// the leader holding 60% against a 50% floor. Its value per share is
// (1000 × 1.1 + 1100) / 1e6 USDC, so the floor leaves room for 200k more
// shares: 440 USDC of outside deposits.
const VAULT: Vault = {
  vaultPubkey: "market:1",
  leader: "Leader111111111111111111111111111111111111",
  tvl: 2200,
  baseReserve: 1000,
  quoteReserve: 1100,
  totalShares: 1_000_000,
  leaderShares: 600_000,
  vps: 1,
  minLeaderSharePpm: 500_000,
  frozen: false,
  outsideDepositsApproved: true,
  chain: {
    index: 0,
    seq: 1n,
    baseAtoms: 1000n * ATOMS,
    quoteAtoms: 1100n * ATOMS,
    totalShares: 1_000_000n,
    leaderShares: 600_000n,
    hwm: 0n,
    perfFeeRatePpm: 0,
    frozen: false,
  },
  refPrice: 1.1,
  quoteUsd: 1,
  usdIndicative: false,
  allowOutsideDepositors: true,
  name: null,
};

// The vault holding only EURC: deposits stay closed until it rebalances.
const ONE_SIDED: Vault = {
  ...VAULT,
  quoteReserve: 0,
  chain: { ...VAULT.chain, quoteAtoms: 0n },
};

const RICH = { base: 10_000n * ATOMS, quote: 10_000n * ATOMS };

// A base-driven deposit of `base` EURC, its quote leg filled the way the
// dialog fills it.
const input = (
  base: string,
  over: Partial<DepositFormInput> = {},
): DepositFormInput => {
  const vault = over.vault ?? VAULT;
  return {
    market: MARKET,
    vault,
    owner: OWNER,
    baseAmount: base,
    quoteAmount: fillOtherLeg(MARKET, vault, "base", base),
    activeLeg: "base",
    knownAtoms: RICH,
    ...over,
  };
};

describe("fillOtherLeg", () => {
  it("fills the derived leg pro-rata at the token's full precision", () => {
    expect(fillOtherLeg(MARKET, VAULT, "base", "100")).toBe("110.000000");
    expect(fillOtherLeg(MARKET, VAULT, "quote", "110")).toBe("100.000000");
  });

  it("blanks the derived leg while the driven one can't be sized", () => {
    expect(fillOtherLeg(MARKET, VAULT, "base", "")).toBe("");
    expect(fillOtherLeg(MARKET, VAULT, "base", "0.000001")).toBe("");
  });
});

describe("sizeLeg", () => {
  it("words the program's rejection of a leg the vault doesn't hold", () => {
    expect(sizeLeg(MARKET, ONE_SIDED, "quote", "100")).toEqual({
      error: "This vault holds no USDC. Enter the EURC amount instead.",
    });
  });
});

describe("depositForm", () => {
  it("accepts a covered basket under the floor and prices it in USD", () => {
    const form = depositForm(input("100"));
    expect(form.sizing?.baseIn).toBe(100n * ATOMS);
    expect(form.sizing?.quoteIn).toBe(110n * ATOMS);
    expect(form.error).toBeNull();
    expect(form.validBasket).toBe(true);
    expect(form.closedReason).toBeNull();
    expect(form.depositBlocked).toBe(false);
    expect(form.floorBreached).toBe(false);
    expect(form.floorPct).toBe(50);
    expect(form.legs.base.usd).toBeCloseTo(110);
    expect(form.legs.quote.usd).toBeCloseTo(110);
    expect(form.usd).toBeCloseTo(220);
  });

  it("reports a sizing rejection and holds the basket invalid", () => {
    const form = depositForm(input("0.000001"));
    expect(form.sizing).toBeNull();
    expect(form.error).toBe(
      "Too small: this amount doesn't buy a single vault share.",
    );
    expect(form.validBasket).toBe(false);
  });

  it("names the leg the wallet falls short on", () => {
    const short = depositForm(
      input("100", { knownAtoms: { base: 50n * ATOMS, quote: RICH.quote } }),
    );
    expect(short.error).toBe("Not enough EURC in your wallet.");
    expect(short.validBasket).toBe(false);

    const shortQuote = depositForm(
      input("100", { knownAtoms: { base: RICH.base, quote: 100n * ATOMS } }),
    );
    expect(shortQuote.error).toBe("Not enough USDC in your wallet.");
    expect(shortQuote.validBasket).toBe(false);
  });

  it("reports no shortfall while the balances are unknown", () => {
    const loading = depositForm(input("100", { knownAtoms: null }));
    expect(loading.sizing).not.toBeNull();
    expect(loading.error).toBeNull();
    expect(loading.validBasket).toBe(false);
    expect(loading.depositBlocked).toBe(false);

    const noWallet = depositForm(
      input("100", { owner: null, knownAtoms: null }),
    );
    expect(noWallet.error).toBeNull();
    expect(noWallet.validBasket).toBe(false);
    expect(noWallet.closedReason).toBe("Connect a wallet to deposit.");
    expect(noWallet.depositBlocked).toBe(true);
  });

  it("flags a basket past the leader floor", () => {
    // 300 EURC pulls 330 USDC: 660 USDC of value against 440 of room.
    const form = depositForm(input("300"));
    expect(form.validBasket).toBe(true);
    expect(form.floorBreached).toBe(true);
    // 150 EURC is 330 USDC of value, inside the room.
    expect(depositForm(input("150")).floorBreached).toBe(false);
  });

  it("never flags the floor on an invalid basket", () => {
    const form = depositForm(input("300", { knownAtoms: null }));
    expect(form.floorBreached).toBe(false);
  });

  it("holds a one-sided vault closed and sizes only its held leg", () => {
    const form = depositForm(input("100", { vault: ONE_SIDED }));
    expect(form.closedReason).toBe(
      "One-sided for now: the vault holds only EURC. Deposits reopen when the book rebalances.",
    );
    expect(form.depositBlocked).toBe(true);
    // The held leg sizes with nothing pulled on the empty one.
    expect(form.sizing?.quoteIn).toBe(0n);
  });

  it("sizes the quote leg when it is the one driven", () => {
    const form = depositForm(
      input("", {
        activeLeg: "quote",
        baseAmount: fillOtherLeg(MARKET, VAULT, "quote", "110"),
        quoteAmount: "110",
      }),
    );
    expect(form.sizing?.baseIn).toBe(100n * ATOMS);
    expect(form.sizing?.quoteIn).toBe(110n * ATOMS);
    expect(form.validBasket).toBe(true);
    expect(form.usd).toBeCloseTo(220);
  });

  it("is idle with nothing entered", () => {
    const form = depositForm(
      input("", { activeLeg: null, baseAmount: "", quoteAmount: "" }),
    );
    expect(form.sizing).toBeNull();
    expect(form.error).toBeNull();
    expect(form.validBasket).toBe(false);
    expect(form.usd).toBe(0);
  });
});
