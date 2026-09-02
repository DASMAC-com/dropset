import { address } from "@solana/kit";
import { describe, expect, it } from "vitest";

import { applySlippage, settlementFor } from "./eclobSwap";
import type { EclobRoute } from "./route";

// Real base58 addresses — `address()` asserts a 32-byte decode, so a
// hand-shaped placeholder is rejected outright.
const BASE = address("2zMqyX4AYCk6mgy5UZ2S7zUaLxwERhK5WjqDzkPPbSpW");
const QUOTE = address("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
const OWNER = address("11111111111111111111111111111112");

/**
 * A route carrying only the fields `settlementFor` reads. Cast because the
 * full `EclobRoute` also carries the market bytes and price encoding, none of
 * which this derivation touches.
 *
 * `outputMint` is a deliberate SENTINEL rather than the realistic base-on-buy
 * / quote-on-sell value. `settlementFor` returns that field verbatim, so
 * supplying the realistic value and then asserting it would only test this
 * fixture's own arithmetic; a third address proves the field is passed through
 * rather than re-derived. The one thing the function actually computes is
 * `inputMint`, which is what the two side tests below pin.
 */
const OUTPUT_SENTINEL = address("EURCeThrvC3KKDyZEvKSXBgx5aBQBZWkozH3F45CH4rU");

const routeFor = (side: "buy" | "sell"): EclobRoute =>
  ({
    side,
    baseMint: BASE,
    quoteMint: QUOTE,
    outputMint: OUTPUT_SENTINEL,
  }) as unknown as EclobRoute;

describe("settlementFor", () => {
  // The derivation that matters. A transposition here is silent and
  // user-visible: both mints belong to the taker, so the balance reader finds
  // both, measures each in the wrong direction, and its coherence check then
  // withholds the amounts on a swap that really filled.
  it("spends the quote leg on a buy", () => {
    expect(settlementFor(routeFor("buy"), OWNER).inputMint).toBe(QUOTE);
  });

  it("spends the base leg on a sell", () => {
    expect(settlementFor(routeFor("sell"), OWNER).inputMint).toBe(BASE);
  });

  it("passes the route's output mint through untouched", () => {
    for (const side of ["buy", "sell"] as const) {
      expect(settlementFor(routeFor(side), OWNER).outputMint).toBe(
        OUTPUT_SENTINEL,
      );
    }
  });

  it("carries the owner through", () => {
    expect(settlementFor(routeFor("buy"), OWNER).owner).toBe(OWNER);
  });
});

describe("applySlippage", () => {
  it("subtracts the tolerance in basis points", () => {
    expect(applySlippage(1_000_000n, 50)).toBe(995_000n);
    expect(applySlippage(1_000_000n, 0)).toBe(1_000_000n);
  });

  // The documented direction: integer division truncates, which LOWERS the
  // floor, so the floor is loose by at most one atom rather than tight. The
  // worked case from the comment — 1001 at 50 bps has an exact floor of
  // 995.995, and this yields 995.
  it("rounds down, leaving the floor loose by at most one atom", () => {
    expect(applySlippage(1001n, 50)).toBe(995n);
    for (const out of [1n, 7n, 999n, 1001n, 123_457n]) {
      const floor = applySlippage(out, 50);
      const exact = (out * 9_950n) / 10_000n;
      expect(floor).toBe(exact);
      expect(floor).toBeLessThanOrEqual(out);
    }
  });

  // The RangeError this guard exists for: Math.trunc(NaN) is NaN, Math.min and
  // Math.max propagate it, and BigInt(NaN) throws — out of a swap that has
  // already been quoted and is about to be signed.
  it("clamps a non-finite tolerance instead of throwing", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -Infinity]) {
      expect(() => applySlippage(1_000n, bad)).not.toThrow();
      expect(applySlippage(1_000n, bad)).toBe(1_000n);
    }
  });

  it("clamps a negative tolerance to zero", () => {
    expect(applySlippage(1_000n, -50)).toBe(1_000n);
  });

  // Capped at 99.99% so minOut stays positive and the swap always carries a
  // real floor, rather than zeroing or overflowing the u64 instruction arg.
  it("caps an absurd tolerance so the floor stays positive", () => {
    expect(applySlippage(1_000_000n, 10_000)).toBe(100n);
    expect(applySlippage(1_000_000n, 1_000_000)).toBe(100n);
  });
});
