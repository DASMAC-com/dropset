import type { SolanaClientRuntime } from "@solana/client";
import type { Signature } from "@solana/kit";
import {
  SWAP_CONFIRM_MAX_UNKNOWN_POLLS,
  SWAP_CONFIRMATION_POLL_MS,
  SWAP_CONFIRMATION_TIMEOUT_MS,
} from "../data/timings";
import { SwapError } from "./types";

// Wallet `sendTransaction` returns after submission, not after the chain has
// confirmed the tx — so balance re-fetches fired immediately after see stale
// data. Poll `getSignatureStatuses` until the signature reaches `confirmed`
// (or `finalized`) and bail with an error on revert or timeout.
//
// Route-neutral: it takes a signature and an rpc, and both swap paths use it.
export async function waitForSwapConfirmation(
  rpc: SolanaClientRuntime["rpc"],
  signature: Signature,
  {
    timeoutMs = SWAP_CONFIRMATION_TIMEOUT_MS,
    pollIntervalMs = SWAP_CONFIRMATION_POLL_MS,
  } = {},
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let unknownPolls = 0;
  while (Date.now() < deadline) {
    const { value } = await rpc.getSignatureStatuses([signature]).send();
    const status = value[0];
    if (status === null) {
      unknownPolls++;
      if (unknownPolls >= SWAP_CONFIRM_MAX_UNKNOWN_POLLS) {
        throw new SwapError(
          "RPC has no record of the submitted signature — the transaction was likely dropped before reaching a leader.",
          "wallet",
        );
      }
      await new Promise((r) => setTimeout(r, pollIntervalMs));
      continue;
    }
    if (status?.err) {
      // `@solana/kit` parses RPC integer fields as BigInt, so a stock
      // JSON.stringify on a TransactionError (e.g. `{ InstructionError:
      // [0, { Custom: 6005 }] }`) throws "Do not know how to serialize a
      // BigInt" and masks the real revert. Coerce BigInts to strings so
      // the on-chain error survives intact.
      const errStr = JSON.stringify(status.err, (_, v) =>
        typeof v === "bigint" ? v.toString() : v,
      );
      throw new SwapError(`Transaction reverted on-chain: ${errStr}`, "wallet");
    }
    const cs = status?.confirmationStatus;
    if (cs === "confirmed" || cs === "finalized") return;
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new SwapError("Timed out waiting for swap confirmation", "wallet");
}
