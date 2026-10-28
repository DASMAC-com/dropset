import { describe, expect, it } from "vitest";

import { INITIAL_QUOTE, type QuoteState, readyOutAmount } from "./quote";

const IN = "in-mint";
const OUT = "out-mint";

const quote = (over: Partial<QuoteState> = {}): QuoteState => ({
  ...INITIAL_QUOTE,
  status: "ok",
  inAmount: 1_000n,
  outAmount: 990n,
  inputMint: IN,
  outputMint: OUT,
  venue: "dflow",
  hasQuote: true,
  ...over,
});

describe("readyOutAmount", () => {
  it("returns the output of an ok quote for this pair and amount", () => {
    expect(readyOutAmount(quote(), IN, OUT, 1_000n)).toBe(990n);
  });

  it("refuses a quote for a different amount (the debounce window)", () => {
    expect(readyOutAmount(quote(), IN, OUT, 2_000n)).toBeNull();
  });

  it("refuses a partial fill", () => {
    expect(
      readyOutAmount(quote({ inAmount: 600n }), IN, OUT, 1_000n),
    ).toBeNull();
  });

  it("refuses a quote for the previous pair", () => {
    expect(readyOutAmount(quote(), OUT, IN, 1_000n)).toBeNull();
  });

  it("refuses an error, rate-limited or empty quote", () => {
    expect(
      readyOutAmount(quote({ status: "error" }), IN, OUT, 1_000n),
    ).toBeNull();
    expect(
      readyOutAmount(quote({ status: "rateLimited" }), IN, OUT, 1_000n),
    ).toBeNull();
    expect(
      readyOutAmount(quote({ outAmount: 0n }), IN, OUT, 1_000n),
    ).toBeNull();
    expect(readyOutAmount(INITIAL_QUOTE, IN, OUT, 1_000n)).toBeNull();
  });
});
