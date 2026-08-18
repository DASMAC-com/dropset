import { encodePrice } from "@dropset/sdk";
import { describe, expect, it } from "vitest";
import { formatPrice, humanPrice, priceFractionDigits } from "./format";

// The TS side of a cross-language pair. `humanPrice` here and `human_price` in
// tui/src/book.rs implement the same conversion against the same encoding, and
// the Rust one carries a regression test for the bug below. This is that test's
// twin, so the two forks cannot drift silently — which they did until there was
// somewhere in frontend/ to put it.
describe("humanPrice", () => {
  // A raw `Price` is an *atoms* ratio, so the displayed rate only coincides
  // with the decoded value when both mints share a decimal count. Every 6-vs-6
  // pair on the board hides that; a 9-decimal base against a 6-decimal quote
  // showed it as a clean factor of 1000.
  it("scales by the mints' decimals, not just the encoded value", () => {
    expect(humanPrice(encodePrice(1.085), 6, 6)).toBeCloseTo(1.085, 9);
    expect(humanPrice(encodePrice(0.00141), 9, 6)).toBeCloseTo(1.41, 9);
  });

  // `quoteForBase` floors, so probing the ratio at one whole base unit hands a
  // 2-decimal token only about two significant figures. Two IDR-scale asks a
  // tick apart then render as the same number — and in the ladder, collide as a
  // duplicate React key.
  it("resolves adjacent levels on a low-decimal base", () => {
    // A 0.000056 quote-per-base rate on a 2-decimal base against a 6-decimal
    // quote encodes as 0.56. One tick up is 0.5615.
    const lo = encodePrice(0.56);
    const hi = encodePrice(0.5615);
    expect(humanPrice(lo, 2, 6)).toBeCloseTo(0.000056, 12);
    expect(humanPrice(hi, 2, 6)).toBeCloseTo(0.00005615, 12);

    // The property that matters: two distinct levels, two distinct rendered
    // prices.
    const render = (p: number) => formatPrice(p, priceFractionDigits(p));
    expect(render(humanPrice(lo, 2, 6))).not.toEqual(
      render(humanPrice(hi, 2, 6)),
    );
  });
});
