// Route-neutral swap vocabulary, shared by both execution paths (the DFlow
// aggregator route and the eCLOB route).
//
// These lived in lib/dflow/dflowSwap.ts, which meant the eCLOB path imported
// its own error type from the competing route's module. Nothing here knows
// about either venue, so it belongs above both.

import type { Blockhash, Signature } from "@solana/kit";

export type SwapOutcome = Submitted & {
  inAmount: bigint;
  outAmount: bigint;
};

export type SwapErrorKind =
  | "network" // fetch threw — likely offline or DNS failure
  // The venue or program rejected the request: an HTTP non-2xx, no market or
  // no liquidity for the pair, a sizing rejection, a closed gate.
  | "api"
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

export type SwapStatus =
  | "idle"
  | "preparing" // building the transaction (DFlow's /order, the eCLOB re-quote)
  | "signing" // tx handed to wallet for signing + submission
  | "confirming" // signature received, polling RPC for on-chain confirmation
  | "success"
  // Confirmed on-chain, but nothing moved: the eCLOB program soft-reverts a
  // fill below `min_out` and still returns a successful transaction, so this
  // is neither a success nor an error. Its own outcome rather than a flag on
  // `result` because the two need different chrome and different follow-up —
  // the user's funds are untouched and the swap is worth retrying.
  //
  // Only the eCLOB path can produce it; the aggregator route reverts in the
  // ordinary way and lands on "error". Shared here because both hooks return
  // one `UseSwap`, so the panel switches on one union either way.
  | "no-fill"
  // Submitted, but confirmation ran out of time without a verdict, so the
  // swap may still land. Not an error: the form must stay locked rather than
  // invite a retry that would be a second real swap. `error` carries the
  // message and the signature to check.
  | "unconfirmed"
  | "error";

// Statuses during which a swap may still move funds: the Swap button stays
// disabled through all of them, whichever venue's hook holds them.
export const isSwapPending = (status: SwapStatus): boolean =>
  status === "preparing" ||
  status === "signing" ||
  status === "confirming" ||
  status === "unconfirmed";

// Augment the bare swap outcome with the from/to symbols captured at the
// moment of execution. Pinning these here (rather than reading from the
// live store in the result banner) means flipping sides after a swap
// can't rewrite history on the success/error chrome.
export type CompletedSwap = SwapOutcome & {
  fromStablecoin: string;
  toStablecoin: string;
};

// What either route's swap hook returns, so the panel can pick one by route
// mode and render it the same way.
//
// Flat shape (not a discriminated union) on purpose: SwapResult reads
// both `status` and `result` to render the success banner with a link to
// the explorer, and the success → idle transition (via reset()) should
// not require re-typing `result`. A strict union would also force the
// confirming → success transition to atomically swap status + result,
// which is harder to reason about than letting React set both in the
// same effect tick.
export type UseSwap = {
  status: SwapStatus;
  result: CompletedSwap | null;
  error: SwapError | null;
  // `quotedOutAmount` is the output the panel showed when Swap was pressed;
  // the executor refuses a fresh price more than the slippage below it.
  execute: (quotedOutAmount: bigint) => Promise<void>;
  reset: () => void;
};

// A transaction's blockhash lifetime: the blockhash it was built on, and the
// last block height at which that blockhash is valid.
export type Lifetime = {
  blockhash: Blockhash;
  lastValidBlockHeight: bigint;
};

// A submitted transaction: its signature, and its lifetime (null when the
// builder could not say) — what `waitForSwapConfirmation` needs to tell "not
// yet" from "dropped".
export type Submitted = {
  signature: Signature;
  lifetime: Lifetime | null;
};

// Common wallets each surface user-rejection with a slightly different
// message. Match conservatively — we'd rather classify a true wallet
// failure as "rejected" (and prompt the user to retry) than classify a
// real cancel as a generic wallet error. The DFlow route hands the wallet
// `sendTransaction`; the eCLOB and vault routes prefer `signTransaction`, with
// `sendTransaction` as the fallback. Either method surfaces the same rejection
// wording.
export const CANCEL_PATTERN =
  /user (?:reject|cancel|denied|declined)|reject(?:ed)?(?: by user| the request)|cancelled in wallet|approval denied|transaction (?:was )?(?:declined|cancelled|rejected)/i;
