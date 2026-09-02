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
//     rather than of how a node chooses to report balances.
//     `collectFillEvents` authenticates each event's *emitting program*, which
//     is what stops an unrelated program forging one; it says nothing about
//     the event's *subject*, so this module filters by taker itself. And an
//     empty event list is only read as a soft revert once the receipt is known
//     to carry the fields the extraction needs — see `canJudgeFills`.
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

import {
  collectFillEvents,
  type EventTransaction,
  eventAccountKeys,
} from "@dropset/sdk";
import type { Signature } from "@solana/kit";
import {
  REALIZED_FILL_MAX_ATTEMPTS,
  SWAP_CONFIRMATION_POLL_MS,
} from "../data/timings";

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

/**
 * One entry's amount, or `null` when it cannot be parsed.
 *
 * `null` rather than `0n` on purpose: a zero would be indistinguishable from a
 * genuinely empty account and would quietly become part of a delta, so a
 * malformed `post` entry beside a well-formed `pre` one would manufacture a
 * negative movement. The caller escalates a `null` to "this reading is not
 * interpretable" instead.
 */
const amountOf = (entry: TokenBalanceEntry): bigint | null => {
  const raw = entry.uiTokenAmount?.amount;
  if (typeof raw !== "string") return null;
  try {
    return BigInt(raw);
  } catch {
    return null;
  }
};

/**
 * Whether this receipt carries the fields {@link collectFillEvents} needs to
 * reach a verdict at all.
 *
 * This gate is the difference between "the program emitted no fills" and "this
 * reader cannot see whether it did", which `collectFillEvents` itself cannot
 * express — it returns an empty array for both. Its three non-soft-revert
 * empty returns are a failed transaction, absent `meta.innerInstructions`, and
 * an unresolvable account-key list (which is also how it fails closed under
 * `jsonParsed` encoding, where the keys arrive as objects that never match a
 * program address).
 *
 * Without this check an RPC that merely omits inner instructions would make
 * every swap read as a soft revert — telling a user their funds did not move
 * when they did, which is the one error that invites them to swap twice.
 */
const canJudgeFills = (tx: RealizedFillTransaction): boolean =>
  tx.meta?.innerInstructions != null && eventAccountKeys(tx) !== null;

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
  let bad = false;
  const walk = (
    entries: readonly TokenBalanceEntry[] | null | undefined,
    sign: bigint,
  ) => {
    for (const entry of entries ?? []) {
      if (entry.mint !== mint || entry.owner !== owner) continue;
      const amount = amountOf(entry);
      // An entry we cannot parse poisons the whole pairing rather than
      // contributing nothing: `seen` is what tells the caller the metadata was
      // interpretable, so setting it while silently dropping a term is exactly
      // the fabrication this function exists to avoid.
      if (amount === null) {
        bad = true;
        continue;
      }
      seen = true;
      delta += sign * amount;
    }
  };
  walk(tx.meta?.postTokenBalances, 1n);
  walk(tx.meta?.preTokenBalances, -1n);
  return { delta, seen: seen && !bad };
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
  for (let attempt = 0; attempt < REALIZED_FILL_MAX_ATTEMPTS; attempt++) {
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
    if (attempt < REALIZED_FILL_MAX_ATTEMPTS - 1) {
      await new Promise((r) => setTimeout(r, SWAP_CONFIRMATION_POLL_MS));
    }
  }
  if (!tx) return null;
  // A transaction carrying an error moved nothing, but it is also not the
  // soft revert this reader is for — the caller's confirmation step has
  // already thrown on it. Refuse to describe it rather than report a zero
  // fill for a swap that failed outright.
  if (tx.meta?.err != null) return null;

  // Refuse to judge a receipt whose event fields are missing. Without this,
  // "the metadata does not carry inner instructions" and "the handler emitted
  // no fills" are the same empty array — see {@link canJudgeFills}.
  if (!canJudgeFills(tx)) return null;

  const out = netDelta(tx, ref.owner, ref.outputMint);
  const inp = netDelta(tx, ref.owner, ref.inputMint);

  // The verdict. No fill events for THIS taker means the handler took its
  // below-floor branch and returned before the transfer section — the soft
  // revert. Read off our own program's emissions rather than off the balances,
  // so it does not depend on whether a node lists token accounts whose balance
  // is unchanged (a soft revert changes none of them).
  //
  // Filtered by taker because the events are transaction-scoped while this
  // reader answers a question about one party: a soft-reverted swap bundled
  // with someone else's real fill must not read as a success. The program-id
  // check inside `collectFillEvents` authenticates the *emitter*, not the
  // subject, so that filtering is this module's job.
  const filled = collectFillEvents(tx).some(
    (event) => String(event.taker) === ref.owner,
  );

  // Positive proof of movement outranks an absent event. If the taker's output
  // balance demonstrably rose, or their input balance demonstrably fell, the
  // swap moved funds whatever the event list says — and the disagreement means
  // this receipt is not describing what this reader thinks it is. Decline,
  // rather than emit a confident "did not fill" against evidence to the
  // contrary.
  //
  // Both legs are checked, not just the output. Each is spoiled
  // independently — one unparseable entry clears `seen` for its own mint only
  // — so reading a single leg would let a malformed output entry skip this
  // gate entirely while the input leg still showed the spend, reaching the
  // very assertion this guard exists to prevent through the side it was not
  // looking at.
  const movedOut = out.seen && out.delta > 0n;
  const movedIn = inp.seen && inp.delta < 0n;
  if (!filled && (movedOut || movedIn)) return null;

  // The input delta is negative when the taker spent, so flip it. A movement
  // in the wrong direction (a negative output, or a positive input) is a
  // contradiction rather than a small number: withhold the amounts entirely so
  // the caller falls back to its quote, instead of reporting a plausible zero
  // that would displace it.
  const directionsCoherent =
    out.seen && inp.seen && out.delta >= 0n && inp.delta <= 0n;
  // A fill that received nothing contradicts the event list just as squarely,
  // so the amounts are untrustworthy even when their directions are fine.
  const agreesWithVerdict = !filled || out.delta > 0n;
  const amounts =
    directionsCoherent && agreesWithVerdict
      ? { inAmount: -inp.delta, outAmount: out.delta }
      : null;

  return { filled, amounts };
}
