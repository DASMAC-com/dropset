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

const OWNER = "TakerOwner1111111111111111111111111111111111";
const IN_MINT = "InMint1111111111111111111111111111111111111";
const OUT_MINT = "OutMint111111111111111111111111111111111111";

const REF: SettlementRef = {
  owner: OWNER,
  inputMint: IN_MINT,
  outputMint: OUT_MINT,
};

const SOME_ADDRESS = address("11111111111111111111111111111112");

/**
 * One real `[tag][discriminator][body]` event-CPI envelope, base58 as the RPC
 * reports inner-instruction data. Built rather than stubbed so these tests
 * exercise the same extraction the recent-fills tape uses — a hand-waved
 * "pretend there was an event" would not prove the verdict is wired to
 * anything.
 */
const fillEventData = (): string => {
  const body = getFillEventEncoder().encode({
    market: SOME_ADDRESS,
    taker: SOME_ADDRESS,
    leader: SOME_ADDRESS,
    quoteAuthority: SOME_ADDRESS,
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

/**
 * A confirmed transaction: `fills` inner instructions carrying real fill
 * events (none for a soft revert), plus whatever token-balance metadata.
 */
const tx = (
  fills: number,
  balances: {
    pre?: ReturnType<typeof balance>[];
    post?: ReturnType<typeof balance>[];
  },
  err?: unknown,
): RealizedFillTransaction => ({
  meta: {
    err,
    innerInstructions: [
      {
        instructions: Array.from({ length: fills }, () => ({
          programIdIndex: 1,
          data: fillEventData(),
        })),
      },
    ],
    preTokenBalances: balances.pre ?? null,
    postTokenBalances: balances.post ?? null,
  },
  transaction: {
    message: {
      accountKeys: [SOME_ADDRESS as string, DROPSET_PROGRAM_ADDRESS as string],
    },
  },
});

/** An rpc whose single `getTransaction` answers with `t`, however called. */
const rpcReturning = (t: RealizedFillTransaction | null) => ({
  getTransaction: () => ({ send: async () => t }),
});

const SIGNATURE = "sig" as never;

describe("readRealizedFill", () => {
  it("reports a fill and the taker's own deltas", async () => {
    const realized = await readRealizedFill(
      rpcReturning(
        tx(1, {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "50"),
          ],
          post: [
            balance(IN_MINT, OWNER, "400"),
            balance(OUT_MINT, OWNER, "1250"),
          ],
        }),
      ),
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
      rpcReturning(
        tx(0, {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "50"),
          ],
          post: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "50"),
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

  // The verdict must not depend on the balance metadata. A node that lists
  // only *changed* token accounts reports nothing at all for a soft revert,
  // which is exactly when the caller most needs the right answer.
  it("still reports no fill when the receipt carries no balance metadata", async () => {
    const realized = await readRealizedFill(
      rpcReturning(tx(0, {})),
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
        tx(1, {
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
    const other = "OtherOwner11111111111111111111111111111111";
    const realized = await readRealizedFill(
      rpcReturning(
        tx(0, {
          pre: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "0"),
            balance(OUT_MINT, other, "7"),
          ],
          post: [
            balance(IN_MINT, OWNER, "1000"),
            balance(OUT_MINT, OWNER, "0"),
            // The market treasury moving is not the taker being filled.
            balance(OUT_MINT, other, "999"),
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
        tx(
          0,
          { pre: [balance(IN_MINT, OWNER, "1000")], post: [] },
          { InstructionError: [0, { Custom: 6005 }] },
        ),
      ),
      SIGNATURE,
      REF,
    );

    expect(realized).toBe(null);
  });
});
