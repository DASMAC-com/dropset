// Route-neutral swap vocabulary, shared by both execution paths (the DFlow
// aggregator route and the eCLOB route).
//
// These lived in lib/dflow/dflowSwap.ts, which meant the eCLOB path imported
// its own error type from the competing route's module. Nothing here knows
// about either venue, so it belongs above both.

import type { Signature } from "@solana/kit";

export type SwapOutcome = Submitted & {
  inAmount: bigint;
  outAmount: bigint;
};

export type SwapErrorKind =
  | "network" // fetch threw — likely offline or DNS failure
  | "api" // /order returned non-2xx
  | "wallet" // wallet adapter failed in a non-user-cancel way
  | "rejected" // user explicitly cancelled in the wallet UI
  // Submitted, but confirmation ran out of time without a verdict: the
  // transaction may still land, so this must not read as safe to retry.
  | "unconfirmed";

export class SwapError extends Error {
  readonly kind: SwapErrorKind;
  readonly httpStatus?: number;
  readonly code?: string;
  // The submitted transaction, when the error came after submission (a
  // revert, an unconfirmed outcome), so the UI can link to it.
  readonly signature?: Signature;
  constructor(
    message: string,
    kind: SwapErrorKind,
    httpStatus?: number,
    code?: string,
    signature?: Signature,
  ) {
    super(message);
    this.name = "SwapError";
    this.kind = kind;
    this.httpStatus = httpStatus;
    this.code = code;
    this.signature = signature;
  }
}

// A submitted transaction: its signature, and the last block height at which
// its blockhash is valid (null when the builder could not say) — what
// `waitForSwapConfirmation` needs to tell "not yet" from "dropped".
export type Submitted = {
  signature: Signature;
  lastValidBlockHeight: bigint | null;
};

// Common wallets each surface user-rejection with a slightly different
// message. Match conservatively — we'd rather classify a true wallet
// failure as "rejected" (and prompt the user to retry) than classify a
// real cancel as a generic wallet error. Both routes hand the wallet the
// same `sendTransaction` and see the same rejection messages.
export const CANCEL_PATTERN =
  /user (?:reject|cancel|denied|declined)|reject(?:ed)?(?: by user| the request)|cancelled in wallet|approval denied|transaction (?:was )?(?:declined|cancelled|rejected)/i;
