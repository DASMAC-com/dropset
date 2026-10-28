import type { Transaction } from "@solana/kit";
import { describe, expect, it } from "vitest";

import { type ParsedDflowOrder, parseDflowOrder } from "../validate";
import { checkDflowOrder } from "./dflowSwap";

const USER = "11111111111111111111111111111112";
const OTHER = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

// Only the signature map is read: its keys are the message's signers, the
// fee payer first.
const txSignedBy = (...signers: string[]) =>
  ({
    messageBytes: new Uint8Array(),
    signatures: Object.fromEntries(signers.map((s) => [s, null])),
  }) as unknown as Transaction;

const order = (over: Partial<ParsedDflowOrder> = {}): ParsedDflowOrder => ({
  transaction: "AA==",
  inAmount: 1_000_000n,
  outAmount: 995_000n,
  slippageBps: 50,
  lastValidBlockHeight: 100n,
  ...over,
});

const request = {
  atomicAmount: 1_000_000n,
  quotedOutAmount: 1_000_000n,
  slippageBps: "50",
  userPublicKey: USER,
};

describe("checkDflowOrder", () => {
  it("accepts an order for the requested amount within slippage, signed by the user alone", () => {
    expect(() =>
      checkDflowOrder(order(), txSignedBy(USER), request),
    ).not.toThrow();
  });

  it("rejects an order for a different input amount", () => {
    expect(() =>
      checkDflowOrder(order({ inAmount: 999_999n }), txSignedBy(USER), request),
    ).toThrow(/different input amount/);
  });

  it("rejects an output below the shown quote less slippage", () => {
    // 50 bps under 1_000_000 is a floor of 995_000.
    expect(() =>
      checkDflowOrder(
        order({ outAmount: 994_999n }),
        txSignedBy(USER),
        request,
      ),
    ).toThrow(/price moved/);
  });

  it("uses the server's slippage under auto", () => {
    const auto = { ...request, slippageBps: "auto" };
    expect(() =>
      checkDflowOrder(
        order({ outAmount: 990_000n, slippageBps: 100 }),
        txSignedBy(USER),
        auto,
      ),
    ).not.toThrow();
    expect(() =>
      checkDflowOrder(
        order({ outAmount: 989_999n, slippageBps: 100 }),
        txSignedBy(USER),
        auto,
      ),
    ).toThrow(/price moved/);
  });

  it("rejects another fee payer or an extra signer", () => {
    expect(() => checkDflowOrder(order(), txSignedBy(OTHER), request)).toThrow(
      /fee payer or signer/,
    );
    expect(() =>
      checkDflowOrder(order(), txSignedBy(USER, OTHER), request),
    ).toThrow(/fee payer or signer/);
  });
});

describe("parseDflowOrder", () => {
  const raw = {
    transaction: "AA==",
    inAmount: "5",
    outAmount: "4",
    slippageBps: 50,
    lastValidBlockHeight: 123,
  };

  it("parses the slippage and the expiry height", () => {
    const parsed = parseDflowOrder(raw);
    expect(parsed.slippageBps).toBe(50);
    expect(parsed.lastValidBlockHeight).toBe(123n);
  });

  it("reads a missing expiry height as null", () => {
    const { lastValidBlockHeight: _, ...rest } = raw;
    expect(parseDflowOrder(rest).lastValidBlockHeight).toBeNull();
  });

  it("rejects a missing or fractional slippage", () => {
    const { slippageBps: _, ...rest } = raw;
    expect(() => parseDflowOrder(rest)).toThrow(/slippageBps/);
    expect(() => parseDflowOrder({ ...raw, slippageBps: 0.5 })).toThrow(
      /slippageBps/,
    );
  });
});
