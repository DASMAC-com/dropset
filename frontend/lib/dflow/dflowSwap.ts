// cspell:word jito
"use client";

import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  getBase64Encoder,
  getTransactionDecoder,
  type SendableTransaction,
  type Signature,
  type Transaction,
} from "@solana/kit";
import {
  DFLOW_ORDER_TIMEOUT_MS,
  SWAP_CONFIRM_MAX_UNKNOWN_POLLS,
  SWAP_CONFIRMATION_POLL_MS,
  SWAP_CONFIRMATION_TIMEOUT_MS,
} from "../data/timings";
import { DFLOW_ORDER_URL, PLATFORM_FEE } from "../env";
import { getErrorMessage } from "../guards";
import {
  type ParsedDflowOrder,
  parseDflowOrder,
  ValidationError,
} from "../validate";
import { resolveFeeAccount } from "./feeVault";

// Resolve the platform-fee parameters for the output mint. Returns null —
// meaning "declare no fee" — unless a fee is configured AND the fee wallet's
// ATA for this mint already exists on-chain. DFlow rejects /order when the
// `feeAccount` is missing, and factors a declared fee into the slippage budget
// even when uncollected, so a mint without a pre-created vault must not
// advertise the fee. Vault existence is resolved (and cached) by feeVault.ts.
async function platformFeeParams(
  rpc: SolanaClientRuntime["rpc"],
  outputMint: string,
): Promise<{ bps: number; feeAccount: string } | null> {
  if (!PLATFORM_FEE) return null;
  const feeAccount = await resolveFeeAccount(rpc, outputMint);
  if (!feeAccount) return null;
  return { bps: PLATFORM_FEE.bps, feeAccount };
}

// DFlow's developer endpoint. URL lives in lib/env.ts (DFLOW_ORDER_URL) so
// dev/prod (or a proxy route handler) can diverge without editing this
// module. Swap path uses `/order` (the unified imperative endpoint)
// because it supports both classic SPL and Token-2022 mints — `/intent`
// doesn't.

export type DflowSwapInput = {
  inputMint: string;
  outputMint: string;
  // Input amount in base units (already scaled by the from-mint's decimals).
  atomicAmount: bigint;
  // Either "auto" (server picks slippage) or a numeric basis-points string.
  slippageBps: string;
  userPublicKey: string;
  walletSession: WalletSession;
  // Used to check (once, cached) whether the platform-fee vault for the output
  // mint exists on-chain before declaring the fee to DFlow.
  rpc: SolanaClientRuntime["rpc"];
};

export type DflowSwapResult = {
  signature: Signature;
  inAmount: bigint;
  outAmount: bigint;
};

export type DflowSwapErrorKind =
  | "network" // fetch threw — likely offline or DNS failure
  | "api" // /order returned non-2xx
  | "wallet" // wallet adapter failed in a non-user-cancel way
  | "rejected"; // user explicitly cancelled in the wallet UI

export class DflowSwapError extends Error {
  readonly kind: DflowSwapErrorKind;
  readonly httpStatus?: number;
  readonly code?: string;
  constructor(
    message: string,
    kind: DflowSwapErrorKind,
    httpStatus?: number,
    code?: string,
  ) {
    super(message);
    this.name = "DflowSwapError";
    this.kind = kind;
    this.httpStatus = httpStatus;
    this.code = code;
  }
}

type OrderErrorBody = {
  code?: string;
  msg?: string;
};

// Extract a human-readable message from a non-2xx DFlow response. Both
// /quote (handled directly) and /order (handled here) wrap errors as
// `{ msg, code }`. Falls back to a status + truncated raw body so a
// transient HTML 502 page surfaces as "HTTP 502: <!DOCTYPE…" rather
// than the generic "HTTP 502" the previous open-coded paths produced.
export type DflowApiErrorInfo = { message: string; code: string | null };
export async function extractDflowApiError(
  res: Response,
): Promise<DflowApiErrorInfo> {
  let bodyText: string | null = null;
  try {
    bodyText = await res.text();
  } catch {
    return { message: `HTTP ${res.status}`, code: null };
  }
  if (!bodyText) return { message: `HTTP ${res.status}`, code: null };
  try {
    const body = JSON.parse(bodyText) as OrderErrorBody;
    if (typeof body?.msg === "string" && body.msg.length > 0) {
      return {
        message: body.msg,
        code: typeof body.code === "string" ? body.code : null,
      };
    }
    if (typeof body?.code === "string" && body.code.length > 0) {
      return { message: `${body.code} (HTTP ${res.status})`, code: body.code };
    }
    return {
      message: `HTTP ${res.status}: ${bodyText.slice(0, MAX_RAW_BODY_PREVIEW)}`,
      code: null,
    };
  } catch {
    return { message: `HTTP ${res.status} with malformed body`, code: null };
  }
}
const MAX_RAW_BODY_PREVIEW = 200;

// Common wallets each surface user-rejection with a slightly different
// message. Match conservatively — we'd rather classify a true wallet
// failure as "rejected" (and prompt the user to retry) than classify a
// real cancel as a generic wallet error. Shared with the eCLOB swap path,
// which hands the wallet the same `sendTransaction` and sees the same
// rejection messages.
export const CANCEL_PATTERN =
  /user (?:reject|cancel|denied|declined)|reject(?:ed)?(?: by user| the request)|cancelled in wallet|approval denied|transaction (?:was )?(?:declined|cancelled|rejected)/i;

// Execute a swap end-to-end:
//   1. GET /order with `allowAsyncExec=false` so DFlow returns a sync single
//      tx (no Jito open-order/fill split — simplest confirm path).
//   2. Base64-decode the returned transaction to a kit Transaction object.
//   3. Hand it to the wallet's `sendTransaction` which signs & submits in one
//      shot, returning the on-chain signature once it reaches `confirmed`.
export async function executeDflowSwap(
  input: DflowSwapInput,
): Promise<DflowSwapResult> {
  const {
    inputMint,
    outputMint,
    atomicAmount,
    slippageBps,
    userPublicKey,
    walletSession,
    rpc,
  } = input;

  const url = new URL(DFLOW_ORDER_URL);
  url.searchParams.set("inputMint", inputMint);
  url.searchParams.set("outputMint", outputMint);
  url.searchParams.set("amount", atomicAmount.toString());
  url.searchParams.set("slippageBps", slippageBps);
  url.searchParams.set("userPublicKey", userPublicKey);
  url.searchParams.set("allowAsyncExec", "false");
  url.searchParams.set("dynamicComputeUnitLimit", "true");

  // Skip fee params entirely when no fee is configured or no fee ATA exists
  // for this output mint. DFlow factors a declared fee into slippage budget
  // even if uncollected, so a missing-ATA mint must not advertise the fee.
  const fee = await platformFeeParams(rpc, outputMint);
  if (fee) {
    url.searchParams.set("platformFeeBps", String(fee.bps));
    url.searchParams.set("feeAccount", fee.feeAccount);
  }

  const timeout = AbortSignal.timeout(DFLOW_ORDER_TIMEOUT_MS);
  let res: Response;
  try {
    res = await fetch(url.toString(), { signal: timeout });
  } catch (e) {
    if (e instanceof DOMException && e.name === "TimeoutError") {
      throw new DflowSwapError("DFlow /order timed out — try again", "network");
    }
    throw new DflowSwapError("Network error reaching DFlow", "network");
  }

  if (!res.ok) {
    const info = await extractDflowApiError(res);
    throw new DflowSwapError(
      info.message,
      "api",
      res.status,
      info.code ?? undefined,
    );
  }

  let order: ParsedDflowOrder;
  try {
    const raw: unknown = await res.json();
    order = parseDflowOrder(raw);
  } catch (e) {
    if (e instanceof ValidationError) {
      throw new DflowSwapError(
        `DFlow returned an invalid order: ${e.message}`,
        "api",
        res.status,
      );
    }
    throw new DflowSwapError(
      "DFlow order response could not be parsed",
      "api",
      res.status,
    );
  }

  let tx: ReturnType<ReturnType<typeof getTransactionDecoder>["decode"]>;
  try {
    const txBytes = getBase64Encoder().encode(order.transaction);
    tx = getTransactionDecoder().decode(txBytes);
  } catch (e) {
    throw new DflowSwapError(
      `DFlow returned an undecodable transaction: ${getErrorMessage(e)}`,
      "api",
      res.status,
    );
  }

  if (!walletSession.sendTransaction) {
    throw new DflowSwapError(
      "Connected wallet doesn't support sendTransaction",
      "wallet",
    );
  }

  let signature: Signature;
  try {
    // Cast: the DFlow tx is missing the user's signature — the wallet adds it
    // during signing. The WalletSession type asks for SendableTransaction
    // (fully signed) but at runtime Wallet Standard adapters happily complete
    // a partially-signed tx before submitting.
    signature = await walletSession.sendTransaction(
      tx as Transaction & SendableTransaction,
      { commitment: "confirmed" },
    );
  } catch (e) {
    const msg = getErrorMessage(e);
    const cancelled = CANCEL_PATTERN.test(msg);
    throw new DflowSwapError(
      cancelled ? "Cancelled in wallet" : msg,
      cancelled ? "rejected" : "wallet",
    );
  }

  return {
    signature,
    inAmount: order.inAmount,
    outAmount: order.outAmount,
  };
}

// Wallet `sendTransaction` returns after submission, not after the chain has
// confirmed the tx — so balance re-fetches fired immediately after see stale
// data. Poll `getSignatureStatuses` until the signature reaches `confirmed`
// (or `finalized`) and bail with an error on revert or timeout.
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
        throw new DflowSwapError(
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
      throw new DflowSwapError(
        `Transaction reverted on-chain: ${errStr}`,
        "wallet",
      );
    }
    const cs = status?.confirmationStatus;
    if (cs === "confirmed" || cs === "finalized") return;
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new DflowSwapError("Timed out waiting for swap confirmation", "wallet");
}
