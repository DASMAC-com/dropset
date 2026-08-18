import { encodePrice, quoteForBase } from "@dropset/sdk";
import { describe, expect, it } from "vitest";
import {
  formatPrice,
  humanPrice,
  PRICE_PROBE_ATOMS,
  priceFractionDigits,
} from "./format";

// `humanPrice` has two halves, and they are covered in two different places.
//
// The decimals scaling is shared with Rust: `atoms_ratio_to_human` in
// util/src/decimals.rs is the same conversion, and its own tests pin the
// two-power association this file's grouping mirrors. The first test below is
// the TS side of that pair.
//
// The *probe* is not shared. `atoms_ratio_to_human` takes an already-computed
// ratio, so the PRICE_PROBE_ATOMS step — the part that stops a low-decimal base
// from flooring adjacent levels onto one number — exists only here. The second
// test is therefore the only coverage of that property anywhere in the repo,
// which is why it spells out the mechanism rather than just asserting values.
describe("humanPrice", () => {
  // A raw `Price` is an *atoms* ratio, so the displayed rate only coincides
  // with the decoded value when both mints share a decimal count. Every 6-vs-6
  // pair on the board hides that; a 9-decimal base against a 6-decimal quote
  // showed it as a clean factor of 1000.
  it("scales by the mints' decimals, not just the encoded value", () => {
    expect(humanPrice(encodePrice(1.085), 6, 6)).toBeCloseTo(1.085, 9);
    expect(humanPrice(encodePrice(0.00141), 9, 6)).toBeCloseTo(1.41, 9);
  });

  // The cross-language contract is the arithmetic ASSOCIATION, not just the
  // value: both forks must scale by 10^base / 10^quote as two separate powers
  // rather than the algebraically equal 10^(base-quote), whose single
  // exponentiation is not correctly rounded — the two panes would otherwise
  // disagree by an ulp on the same level. util/src/decimals.rs pins this on
  // the Rust side with `the_scaling_is_grouped_as_two_powers`; this is its
  // counterpart, and it is what the audit registry's util <-> frontend entry
  // asks for.
  //
  // The pair list mirrors the Rust test's, but only ONE of them actually
  // discriminates: at 6/9 the two forms differ in the last ulp, and at 6/6,
  // 9/6, 2/6 and 8/6 they are bit-identical. So 6/9 is the whole test — do not
  // drop it as redundant, and keep a base < quote pair in any list that
  // replaces this one.
  it("groups the scaling as two powers", () => {
    const pairs: [number, number][] = [
      [6, 6],
      [9, 6],
      [2, 6],
      [6, 9],
      [8, 6],
    ];
    for (const [base, quote] of pairs) {
      const bits = encodePrice(1.2345678912345);
      const ratio =
        Number(quoteForBase(bits, PRICE_PROBE_ATOMS)) /
        Number(PRICE_PROBE_ATOMS);
      expect(humanPrice(bits, base, quote)).toBe(
        (ratio * 10 ** base) / 10 ** quote,
      );
    }
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
