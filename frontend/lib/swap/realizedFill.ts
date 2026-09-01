// What a confirmed swap transaction *actually* moved, read back off the chain
// rather than taken from the pre-flight simulation.
//
// This exists because on our own program a failed swap is not a failed
// transaction. When the fill lands below the taker's `min_out` the handler
// soft-reverts: it restores inventory, level sizes, the flush bit, the nonce
// and the fee accruals, transfers nothing, emits no fill events, and returns
// `Ok`. The transaction therefore succeeds, its signature status carries no
// error, and every check that stops at "did the transaction revert?" — which
// is what `waitForSwapConfirmation` answers — says the swap went through. A
// caller that then reports its own quoted figures shows the user a completed
// swap at a price nothing traded at.
//
// Two different questions, answered from two different parts of the receipt,
// because they have different failure modes:
//
//   * **Did it fill?** — from the fill events. The program emits one per leg
//     via event CPI on a real fill and *none* at all when it soft-reverts, so
//     their absence is the signal, and it is a property of our own program
//     rather than of how a node chooses to report balances. `collectFillEvents`
//     verifies each event's emitting program, so this cannot be spoofed by an
//     unrelated instruction in the same transaction.
//   * **How much?** — from the taker's own token-balance delta, which is the
//     quantity `min_out` is itself expressed in: the floor is sized on what
//     lands in the taker's account after both fees (see `eclobSwap`'s quote),
//     so reading the same quantity back keeps the floor and the report in one
//     unit. Recovering it from the events instead would mean summing per-leg
//     fills and re-netting the platform fee — arithmetic the delta already did.
//
// Splitting them this way keeps the verdict robust when the amounts are not
// recoverable: balance metadata is reported per node and this makes no
// assumption about whether an *unchanged* account is listed, whereas a
// verdict read off the deltas would have to.
//
// Route-neutral by construction as far as the balances go; the event check is
// specific to our program, which is also the only route that soft-reverts —
// the aggregator route reverts in the ordinary way and never reaches here.

import { collectFillEvents, type EventTransaction } from "@dropset/sdk";
import type { Signature } from "@solana/kit";
import { SWAP_CONFIRMATION_POLL_MS } from "../data/timings";

/**
 * Minimal structural shape of the one RPC method this needs, declared here
 * rather than imported for the same reason `chainClock` declares `BlockTimeRpc`
 * — the module stays independent of which client the caller holds.
 */
export type TransactionRpc = {
  getTransaction: (...args: never[]) => {
    send: () => Promise<RealizedFillTransaction | null>;
  };
};

/** One entry of a transaction's pre/post token-balance metadata. */
type TokenBalanceEntry = {
  mint?: string;
  owner?: string;
  uiTokenAmount?: { amount?: string };
};

/**
 * The slice of a confirmed transaction this reads: what the SDK's event
 * extraction needs, plus the two token-balance arrays.
 */
export type RealizedFillTransaction = EventTransaction & {
  meta?: {
    preTokenBalances?: readonly TokenBalanceEntry[] | null;
    postTokenBalances?: readonly TokenBalanceEntry[] | null;
  } | null;
};

/**
 * Who settled, and in which mints. Both mints are **on-chain** terms (the mock
 * demo mints on localnet), because that is what the transaction metadata
 * reports — a display-mint match would find nothing there.
 */
export type SettlementRef = {
  owner: string;
  inputMint: string;
  outputMint: string;
};

/** What the transaction actually did. */
export type RealizedFill = {
  /**
   * Whether the program filled at all. `false` is the soft revert: a confirmed,
   * error-free transaction that moved nothing.
   */
  filled: boolean;
  /**
   * Realized movement in atoms of each mint, or `null` when the receipt did
   * not describe the taker's mints — a caller that still needs figures then
   * falls back to its own quote rather than inventing one.
   */
  amounts: { inAmount: bigint; outAmount: bigint } | null;
};

// A transaction is readable by signature status a beat before `getTransaction`
// will return it, so a first null is ordinary rather than an answer. Bounded
// tightly: the point is to ride out that beat, not to wait out a node that
// has genuinely lost the transaction.
const MAX_ATTEMPTS = 4;

const amountOf = (entry: TokenBalanceEntry): bigint => {
  const raw = entry.uiTokenAmount?.amount;
  if (typeof raw !== "string") return 0n;
  try {
    return BigInt(raw);
  } catch {
    return 0n;
  }
};

/**
 * Net movement of `mint` across `owner`'s token accounts, and whether the
 * metadata described that pairing at all.
 *
 * The `seen` half is what separates "moved nothing" from "cannot tell". A
 * balance that did not change still appears in both arrays, and an account
 * created by this very transaction appears only in `post` (with a zero
 * balance on a soft revert), so a genuine no-fill is always *seen* with a
 * zero delta. Nothing at all means metadata this reader can't interpret.
 */
const netDelta = (
  tx: RealizedFillTransaction,
  owner: string,
  mint: string,
): { delta: bigint; seen: boolean } => {
  let delta = 0n;
  let seen = false;
  const walk = (
    entries: readonly TokenBalanceEntry[] | null | undefined,
    sign: bigint,
  ) => {
    for (const entry of entries ?? []) {
      if (entry.mint !== mint || entry.owner !== owner) continue;
      seen = true;
      delta += sign * amountOf(entry);
    }
  };
  walk(tx.meta?.postTokenBalances, 1n);
  walk(tx.meta?.preTokenBalances, -1n);
  return { delta, seen };
};

/**
 * Read what a confirmed swap actually did for `ref.owner`.
 *
 * Returns `null` only when the receipt cannot be read at all — deliberately
 * distinct from `filled: false`, so a caller keeps quoting its own figures
 * rather than announcing a swap that did not happen on the strength of an
 * unreadable receipt. A readable soft revert is the case this exists for, and
 * that one comes back as `filled: false`.
 *
 * Call it only after the signature has confirmed; it does not wait for
 * confirmation, only for the metadata to become fetchable.
 */
export async function readRealizedFill(
  rpc: TransactionRpc,
  signature: Signature,
  ref: SettlementRef,
): Promise<RealizedFill | null> {
  let tx: RealizedFillTransaction | null = null;
  for (let attempt = 0; attempt < MAX_ATTEMPTS; attempt++) {
    try {
      // `json` rather than `jsonParsed`: the token-balance metadata is the
      // same either way, and the parsed encoding reshapes account keys in a
      // way the rest of the codebase's decoders (see the SDK's
      // `eventAccountKeys`) deliberately refuse to read.
      tx = await rpc
        .getTransaction(
          signature as never,
          {
            commitment: "confirmed",
            encoding: "json",
            maxSupportedTransactionVersion: 0,
          } as never,
        )
        .send();
    } catch {
      // A transport hiccup is worth one more attempt; an exhausted loop falls
      // through to the null return, which the caller reads as "cannot tell".
      tx = null;
    }
    if (tx) break;
    if (attempt < MAX_ATTEMPTS - 1) {
      await new Promise((r) => setTimeout(r, SWAP_CONFIRMATION_POLL_MS));
    }
  }
  if (!tx) return null;
  // A transaction carrying an error moved nothing, but it is also not the
  // soft revert this reader is for — the caller's confirmation step has
  // already thrown on it. Refuse to describe it rather than report a zero
  // fill for a swap that failed outright.
  if (tx.meta?.err != null) return null;

  // The verdict. No fill events means the handler took its below-floor branch
  // and returned before the transfer section — the soft revert. This is read
  // off our own program's emissions rather than off the balances, so it does
  // not depend on whether a node lists token accounts whose balance is
  // unchanged (a soft revert changes none of them).
  const filled = collectFillEvents(tx).length > 0;

  const out = netDelta(tx, ref.owner, ref.outputMint);
  const inp = netDelta(tx, ref.owner, ref.inputMint);
  // The input delta is negative when the taker spent, so flip it. Both are
  // clamped at zero: a negative output (or a positive input) would mean the
  // transaction moved the taker's balance the wrong way, which this reader
  // has no vocabulary for and must not report as a fill size.
  const amounts =
    out.seen && inp.seen
      ? {
          inAmount: inp.delta < 0n ? -inp.delta : 0n,
          outAmount: out.delta > 0n ? out.delta : 0n,
        }
      : null;

  return { filled, amounts };
}
