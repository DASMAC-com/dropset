import { type BookLevel, encodePrice } from "@dropset/sdk";
import { describe, expect, it } from "vitest";
import { levelsToRows, MAX_ROWS } from "./ladder";

// The engine hands a flipped counter back as an ordinary level on the
// opposite side (sdk/ts/src/market.test.ts pins that seam). What is left for
// the view is that a flip landing on a price already quoted reads as one
// level, and that a fully flipped vault still fits on the ladder.
describe("levelsToRows", () => {
  const p = (v: number) => encodePrice(v);

  it("folds an own level and a flipped counter at one price into one row", () => {
    // Best-first asks: a flip at 1.0851 shares its price with an own ask,
    // and the engine reports the two adjacent, own counter first.
    const asks: BookLevel[] = [
      { price: p(1.0851), size: 300n },
      { price: p(1.0851), size: 200n },
      { price: p(1.0904), size: 1_000n },
    ];
    const rows = levelsToRows(asks, 6, 6);
    expect(rows.map((r) => [r.bits, r.size, r.total])).toEqual([
      [p(1.0851), 500n, 500n],
      [p(1.0904), 1_000n, 1_500n],
    ]);
  });

  it("keeps every price of a fully flipped side", () => {
    // One vault's 8 own levels and 8 flipped counters, all distinct prices:
    // the cap must not cut the deeper half of the book. Sized from that
    // protocol fact (2 × N_LEVELS), not from the cap under test.
    const levels: BookLevel[] = Array.from({ length: 2 * 8 }, (_, i) => ({
      price: p(1.08 + i * 0.0001),
      size: 1n,
    }));
    const rows = levelsToRows(levels, 6, 6);
    expect(rows).toHaveLength(16);
    expect(rows.at(-1)?.total).toBe(16n);
  });

  it("caps distinct prices but still folds into the last row", () => {
    const levels: BookLevel[] = Array.from({ length: MAX_ROWS }, (_, i) => ({
      price: p(1.08 + i * 0.0001),
      size: 1n,
    }));
    const last = levels.at(-1);
    if (!last) throw new Error("fixture is empty");
    levels.push({ price: last.price, size: 4n });
    levels.push({ price: p(1.1), size: 9n });
    const rows = levelsToRows(levels, 6, 6);
    expect(rows).toHaveLength(MAX_ROWS);
    expect(rows.at(-1)?.size).toBe(5n);
  });
});
