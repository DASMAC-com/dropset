import type { SolanaClientRuntime } from "@solana/client";
import type { Signature } from "@solana/kit";
import {
  SWAP_CONFIRMATION_POLL_MS,
  SWAP_CONFIRMATION_TIMEOUT_MS,
} from "../data/timings";
import { type Lifetime, SwapError } from "./types";

type Rpc = SolanaClientRuntime["rpc"];
type SignatureStatus = NonNullable<
  Awaited<
    ReturnType<ReturnType<Rpc["getSignatureStatuses"]>["send"]>
  >["value"][number]
>;

export type ConfirmationOptions = {
  // The transaction's blockhash and the last block height at which it is
  // valid. Past that height the transaction can never land, so expiry is the
  // one point at which "the RPC has no record" means "dropped" rather than
  // "not yet". Null when the submitter could not say, in which case nothing
  // is ever declared dropped and an unseen signature runs to the timeout's
  // "unknown" outcome.
  lifetime: Lifetime | null;
  // What the transaction is, for the messages: "swap", "deposit", …
  noun?: string;
  timeoutMs?: number;
  pollIntervalMs?: number;
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// One RPC read. A throw — the fallback provider failing too, after lib/rpc.ts
// failover — is "no answer this poll", never a verdict: a failed read says
// nothing about the transaction, and treating it as a failure would re-arm the
// caller's form for a retry.
const attempt = async <T>(read: () => Promise<T>): Promise<T | null> => {
  try {
    return await read();
  } catch {
    return null;
  }
};

// Whether an unseen transaction is provably expired. "dropped" only on proof
// from one point in chain time: lib/rpc.ts fails over per call, so each read
// below can reach a different provider, and an up-to-date height paired with a
// lagging node's history could otherwise call a landed transaction dropped.
// The finalized height and slot are read together; the blockhash must be
// invalid on a node that has reached that slot (`minContextSlot` makes a
// lagging node refuse, which reads as pending); and the full-history lookup
// counts only from a node at or past it. The blockhash check also means a
// too-low expiry height from the builder (the DFlow route takes it from the
// order response) cannot by itself declare a live transaction dropped. Every
// failed or lagging read answers "pending", and the timeout bounds the wait.
// A transaction found in the history is returned for the caller to judge.
async function expiryVerdict(
  rpc: Rpc,
  signature: Signature,
  lifetime: Lifetime,
): Promise<"pending" | "dropped" | SignatureStatus> {
  const epoch = await attempt(() =>
    rpc.getEpochInfo({ commitment: "finalized" }).send(),
  );
  if (epoch === null || epoch.blockHeight <= lifetime.lastValidBlockHeight) {
    return "pending";
  }
  const at = epoch.absoluteSlot;
  const valid = await attempt(() =>
    rpc
      .isBlockhashValid(lifetime.blockhash, {
        commitment: "finalized",
        minContextSlot: at,
      })
      .send(),
  );
  if (valid === null || valid.value) return "pending";
  const history = await attempt(() =>
    rpc
      .getSignatureStatuses([signature], { searchTransactionHistory: true })
      .send(),
  );
  if (history === null || history.context.slot < at) return "pending";
  return history.value[0] ?? "dropped";
}

// Wallet `sendTransaction` returns after submission, not after the chain has
// confirmed the tx — so balance re-fetches fired immediately after see stale
// data. Poll `getSignatureStatuses` until the signature reaches `finalized`.
//
// The one rule this keeps: never report a failure for a transaction that can
// still land, because every caller re-arms its form on failure and the retry
// would be a second real transaction. So:
//   * an unseen signature is pending until it is provably expired (see
//     `expiryVerdict`) — only then is it dropped. A handful of null polls
//     proves nothing: the send and the poll can go to different providers;
//   * a failed RPC read is skipped, not reported;
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
    lifetime,
    noun = "swap",
    timeoutMs = SWAP_CONFIRMATION_TIMEOUT_MS,
    pollIntervalMs = SWAP_CONFIRMATION_POLL_MS,
  }: ConfirmationOptions,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let landed = false;
  while (Date.now() < deadline) {
    const recent = await attempt(() =>
      rpc.getSignatureStatuses([signature]).send(),
    );
    let status = recent?.value[0] ?? null;
    if (recent !== null && status === null && !landed && lifetime !== null) {
      const verdict = await expiryVerdict(rpc, signature, lifetime);
      if (verdict === "dropped") {
        throw new SwapError(
          `The ${noun} expired before it reached the chain — nothing moved.`,
          "wallet",
        );
      }
      if (verdict !== "pending") status = verdict;
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
