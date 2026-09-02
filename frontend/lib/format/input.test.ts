import { describe, expect, it } from "vitest";

import { parseSlippagePercent, sanitizeSlippagePercent } from "./input";

describe("parseSlippagePercent", () => {
  // The regression. A 0% tolerance is the "exact or nothing" setting, and
  // rejecting it left the input box showing 0.00 while the store kept the
  // previous value — so the swap ran at a tolerance the user had not set and
  // the UI showed one they had.
  it("commits zero rather than rejecting it", () => {
    expect(parseSlippagePercent("0")).toBe(0);
    expect(parseSlippagePercent("0.0")).toBe(0);
    expect(parseSlippagePercent("0.00")).toBe(0);
  });

  it("commits ordinary values", () => {
    expect(parseSlippagePercent("0.01")).toBe(0.01);
    expect(parseSlippagePercent("0.5")).toBe(0.5);
    expect(parseSlippagePercent("50")).toBe(50);
    // Slippage is deliberately uncapped at 100 (see sanitizeSlippagePercent);
    // the on-chain floor is clamped separately by applySlippage.
    expect(parseSlippagePercent("150")).toBe(150);
  });

  // Nothing to commit yet — the box is empty, or mid-type on the way to a
  // real value. These must leave the store alone rather than write a zero.
  it("commits nothing for an input carrying no value", () => {
    expect(parseSlippagePercent("")).toBe(null);
    expect(parseSlippagePercent(".")).toBe(null);
  });

  // The two compose: the sanitizer runs first, so the parser only ever sees
  // digits and at most one dot.
  it("accepts whatever the sanitizer produces", () => {
    for (const raw of ["0", "0.00", "abc0.5xyz", "1..2", "0.999"]) {
      const cleaned = sanitizeSlippagePercent(raw);
      const parsed = parseSlippagePercent(cleaned);
      if (cleaned === "") {
        expect(parsed).toBe(null);
      } else {
        expect(parsed).not.toBe(null);
        expect(parsed).toBeGreaterThanOrEqual(0);
      }
    }
  });
});
