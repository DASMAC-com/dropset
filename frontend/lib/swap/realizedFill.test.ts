import {
  DROPSET_PROGRAM_ADDRESS,
  EVENT_DISCRIMINATOR_LEN,
  EVENT_IX_TAG_LE,
  FILL_EVENT_DISCRIMINATOR,
  getFillEventEncoder,
} from "@dropset/sdk";
import { address, getBase58Decoder } from "@solana/kit";
import { describe, expect, it } from "vitest";

import {
  type RealizedFillTransaction,
  readRealizedFill,
  type SettlementRef,
} from "./realizedFill";

// Real base58 addresses: the event encoder writes `taker` as a 32-byte pubkey,
// so the value the reader compares against `ref.owner` has to round-trip.
const OWNER = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const OTHER_OWNER = "11111111111111111111111111111112";
const IN_MINT = "2zMqyX4AYCk6mgy5UZ2S7zUaLxwERhK5WjqDzkPPbSpW";
const OUT_MINT = "EURCeThrvC3KKDyZEvKSXBgx5aBQBZWkozH3F45CH4rU";

const REF: SettlementRef = {
  owner: OWNER,
  inputMint: IN_MINT,
  outputMint: OUT_MINT,
};

/**
 * One real `[tag][discriminator][body]` event-CPI envelope, base58 as the RPC
 * reports inner-instruction data. Built rather than stubbed so these tests
 * exercise the same extraction the recent-fills tape uses — a hand-waved
 * "pretend there was an event" would not prove the verdict is wired to
 * anything.
 */
const fillEventData = (taker: string = OWNER): string => {
  const body = getFillEventEncoder().encode({
    market: address(OTHER_OWNER),
    taker: address(taker),
    leader: address(OTHER_OWNER),
    quoteAuthority: address(OTHER_OWNER),
    side: 1,
    pad: new Uint8Array(7),
    sectorIdx: 0,
    levelIdx: 0,
    fillBase: 1_200n,
    fillQuote: 600n,
    fillPrice: 0x0001_0000,
    pad2: new Uint8Array(4),
    baseAtomsAfter: 0n,
    quoteAtomsAfter: 0n,
    nonceAfter: 1n,
    takerFeeAtoms: 0n,
  });
  const out = new Uint8Array(
    EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN + body.length,
  );
  out.set(EVENT_IX_TAG_LE, 0);
  out.set(FILL_EVENT_DISCRIMINATOR, EVENT_IX_TAG_LE.length);
  out.set(body, EVENT_IX_TAG_LE.length + EVENT_DISCRIMINATOR_LEN);
  return getBase58Decoder().decode(out);
};

const balance = (mint: string, owner: string, amount: string) => ({
  mint,
  owner,
  uiTokenAmount: { amount },
});

type Balances = {
  pre?: ReturnType<typeof balance>[];
  post?: ReturnType<typeof balance>[];
};

/**
 * A confirmed transaction carrying `takers.length` fill events (none for a
 * soft revert), each attributed to the named taker, plus whatever token-balance
 * metadata.
 */
const tx = (
  takers: string[],
  balances: Balances,
  overrides: Partial<{ err: unknown; noInner: boolean; noKeys: boolean }> = {},
): RealizedFillTransaction => ({
  meta: {
    err: overrides.err,
    innerInstructions: overrides.noInner
      ? null
      : [
          {
            instructions: takers.map((t) => ({
              programIdIndex: 1,
              data: fillEventData(t),
            })),
          },
        ],
    preTokenBalances: balances.pre ?? null,
    postTokenBalances: balances.post ?? null,
  },
  transaction: {
    message: {
      accountKeys: overrides.noKeys
        ? undefined
        : [OTHER_OWNER, DROPSET_PROGRAM_ADDRESS as string],
    },
  },
});

/** An rpc whose single `getTransaction` answers with `t`, however called. */
const rpcReturning = (t: RealizedFillTransaction | null) => ({
  getTransaction: () => ({ send: async () => t }),
});

const SIGNATURE = "sig" as never;

const FILLED: Balances = {
  pre: [balance(IN_MINT, OWNER, "1000"), balance(OUT_MINT, OWNER, "50")],
  post: [balance(IN_MINT, OWNER, "400"), balance(OUT_MINT, OWNER, "1250")],
};
const UNMOVED: Balances = {
  pre: [balance(IN_MINT, OWNER, "1000"), balance(OUT_MINT, OWNER, "50")],
  post: [balance(IN_MINT, OWNER, "1000"), balance(OUT_MINT, OWNER, "50")],
};

describe("readRealizedFill", () => {
  it("reports a fill and the taker's own deltas", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([OWNER], FILLED)),
      SIGNATURE,
      REF,
    );

    // Spent 600 of the input, received 1200 of the output. The input delta is
    // negative on chain and is reported as a positive spend.
    expect(realized).toEqual({
      filled: true,
      amounts: { inAmount: 600n, outAmount: 1200n },
    });
  });

  // The headline case. A fill below `min_out` soft-reverts: the transaction
  // succeeds, carries no error, emits no fill events, and moves nothing. The
  // verdict has to come back false so the caller can say "did not fill"
  // instead of reporting its own quoted figures as a completed swap.
  it("reports no fill when a soft revert emitted no events", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([], UNMOVED)),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({
      filled: false,
      amounts: { inAmount: 0n, outAmount: 0n },
    });
  });

  // The verdict must not depend on the balance metadata. A node that lists
  // only *changed* token accounts reports nothing at all for a soft revert,
  // which is exactly when the caller most needs the right answer.
  it("still reports no fill when the receipt carries no balance metadata", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([], {})),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({ filled: false, amounts: null });
  });

  // A first-time swap into a token creates the output ATA in the same
  // transaction, so it appears only in `post`.
  it("treats an output account created by this transaction as a zero pre-balance", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx([OWNER], {
          pre: [balance(IN_MINT, OWNER, "1000")],
          post: [balance(IN_MINT, OWNER, "0"), balance(OUT_MINT, OWNER, "900")],
        }),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({
      filled: true,
      amounts: { inAmount: 1000n, outAmount: 900n },
    });
  });

  it("ignores another owner's balances on the same mints", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx([], {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "0"),
            balance(OUT_MINT, OTHER_OWNER, "7"),
          ],
          post: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "0"),
            // The market treasury moving is not the taker being filled.
            balance(OUT_MINT, OTHER_OWNER, "999"),
          ],
        }),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({
      filled: false,
      amounts: { inAmount: 0n, outAmount: 0n },
    });
  });

  // "Cannot tell" must stay distinct from "did not fill" — an unreadable
  // receipt has to leave the caller on its quoted figures rather than let it
  // announce a swap that did not happen.
  it("returns null when the transaction cannot be read", async () => {
    expect(await readRealizedFill(rpcReturning(null), SIGNATURE, REF)).toBe(
      null,
    );
  });

  // A transaction that reverted outright is not the soft revert this reader
  // describes; the caller's confirmation step has already thrown on it.
  it("refuses to describe a failed transaction", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx([], UNMOVED, { err: { InstructionError: [0, { Custom: 6005 }] } }),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toBe(null);
  });

  // The blocking case this reader was reworked for. `collectFillEvents`
  // returns an empty array both when the program emitted nothing AND when the
  // receipt lacks the fields it needs, so an RPC that merely omits inner
  // instructions would otherwise turn every real fill into a confident "no
  // funds were swapped" — the one error that invites a duplicate swap.
  it("refuses to judge a receipt with no inner instructions", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([], FILLED, { noInner: true })),
      SIGNATURE,
      REF,
    );

    expect(realized).toBe(null);
  });

  it("refuses to judge a receipt whose account keys cannot be resolved", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([], FILLED, { noKeys: true })),
      SIGNATURE,
      REF,
    );

    expect(realized).toBe(null);
  });

  // The events are transaction-scoped; this reader answers a question about
  // one party. A soft-reverted swap bundled with someone else's real fill must
  // not read as a success.
  it("ignores a fill event belonging to a different taker", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([OTHER_OWNER], UNMOVED)),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({
      filled: false,
      amounts: { inAmount: 0n, outAmount: 0n },
    });
  });

  // Positive proof of receipt outranks an absent event: if the output balance
  // demonstrably rose, declining to answer beats asserting "did not fill"
  // against evidence of funds moving.
  it("declines rather than deny a fill the balances contradict", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([], FILLED)),
      SIGNATURE,
      REF,
    );

    expect(realized).toBe(null);
  });

  // A movement in the wrong direction is a contradiction, not a small number:
  // withhold the amounts so the caller falls back to its quote rather than
  // displacing it with a plausible zero.
  it("withholds amounts when the taker's output balance fell", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx([OWNER], {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "50"),
          ],
          post: [
            balance(IN_MINT, OWNER, "400"),
            balance(OUT_MINT, OWNER, "10"),
          ],
        }),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({ filled: true, amounts: null });
  });

  // A fill that received nothing contradicts the event list just as squarely.
  it("withholds amounts when a fill received nothing", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx([OWNER], UNMOVED)),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({ filled: true, amounts: null });
  });

  // An unparseable entry must clear `seen` rather than contribute a zero,
  // which would manufacture a delta out of a reading this module cannot
  // interpret.
  it("withholds amounts when a balance entry cannot be parsed", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx([OWNER], {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "50"),
          ],
          post: [
            balance(IN_MINT, OWNER, "400"),
            balance(OUT_MINT, OWNER, "not-a-number"),
          ],
        }),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toEqual({ filled: true, amounts: null });
  });
});
