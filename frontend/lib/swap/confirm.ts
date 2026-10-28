import type { SolanaClientRuntime } from "@solana/client";
import type { Signature } from "@solana/kit";
import {
  SWAP_CONFIRMATION_POLL_MS,
  SWAP_CONFIRMATION_TIMEOUT_MS,
} from "../data/timings";
import { SwapError } from "./types";

type Rpc = SolanaClientRuntime["rpc"];

export type ConfirmationOptions = {
  // The last block height at which the transaction's blockhash is valid. Past
  // it the transaction can never land, so it is the one point at which "the
  // RPC has no record" means "dropped" rather than "not yet". Null when the
  // submitter could not say, in which case nothing is ever declared dropped
  // and an unseen signature runs to the timeout's "unknown" outcome.
  lastValidBlockHeight: bigint | null;
  // What the transaction is, for the messages: "swap", "deposit", …
  noun?: string;
  timeoutMs?: number;
  pollIntervalMs?: number;
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// Wallet `sendTransaction` returns after submission, not after the chain has
// confirmed the tx — so balance re-fetches fired immediately after see stale
// data. Poll `getSignatureStatuses` until the signature reaches `finalized`.
//
// The one rule this keeps: never report a failure for a transaction that can
// still land, because every caller re-arms its form on failure and the retry
// would be a second real transaction. So:
//   * an unseen signature is pending until the chain's block height passes
//     the blockhash's `lastValidBlockHeight` — only then is it dropped. A
//     handful of null polls proves nothing: the send and the poll can go to
//     different providers (lib/rpc.ts failover), and a lagging node reads a
//     lower height, which errs towards waiting;
//   * once the signature has reached `confirmed` it has landed, and running
//     out of time before `finalized` is a success, not a failure;
//   * running out of time otherwise is neither: it throws an "unconfirmed"
//     SwapError carrying the signature, which callers surface as "check the
//     explorer" and must not treat as safe to retry.
//
// Route-neutral: it takes a signature and an rpc, and both swap paths and the
// vault path use it.
export async function waitForSwapConfirmation(
  rpc: Rpc,
  signature: Signature,
  {
    lastValidBlockHeight,
    noun = "swap",
    timeoutMs = SWAP_CONFIRMATION_TIMEOUT_MS,
    pollIntervalMs = SWAP_CONFIRMATION_POLL_MS,
  }: ConfirmationOptions,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let landed = false;
  while (Date.now() < deadline) {
    const { value } = await rpc.getSignatureStatuses([signature]).send();
    let status = value[0];
    if (status === null && !landed && lastValidBlockHeight !== null) {
      // Read at `finalized`, which trails the tip: a height that is too low
      // only delays the verdict, never brings it forward.
      const height = await rpc
        .getBlockHeight({ commitment: "finalized" })
        .send();
      if (height > lastValidBlockHeight) {
        // Expired. One last look through the node's full history before
        // calling it, in case the transaction landed in the final valid
        // blocks and has already aged out of the recent-status cache.
        const { value: final } = await rpc
          .getSignatureStatuses([signature], { searchTransactionHistory: true })
          .send();
        status = final[0];
        if (status === null) {
          throw new SwapError(
            `The ${noun} expired before it reached the chain — nothing moved.`,
            "wallet",
          );
        }
      }
    }
    if (status === null) {
      await sleep(pollIntervalMs);
      continue;
    }
    if (status.err) {
      // `@solana/kit` parses RPC integer fields as BigInt, so a stock
      // JSON.stringify on a TransactionError (e.g. `{ InstructionError:
      // [0, { Custom: 6005 }] }`) throws "Do not know how to serialize a
      // BigInt" and masks the real revert. Coerce BigInts to strings so
      // the on-chain error survives intact.
      const errStr = JSON.stringify(status.err, (_, v) =>
        typeof v === "bigint" ? v.toString() : v,
      );
      throw new SwapError(
        `Transaction reverted on-chain: ${errStr}`,
        "wallet",
        undefined,
        undefined,
        signature,
      );
    }
    const cs = status.confirmationStatus;
    if (cs === "finalized") return;
    if (cs === "confirmed") landed = true;
    await sleep(pollIntervalMs);
  }
  if (landed) return;
  throw new SwapError(
    `Couldn't confirm the ${noun} in time — it may still land. Check the explorer before trying again.`,
    "unconfirmed",
    undefined,
    undefined,
    signature,
  );
}
