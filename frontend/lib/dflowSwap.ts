"use client";

import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  type Address,
  address,
  getBase64Encoder,
  getTransactionDecoder,
  type SendableTransaction,
  type Signature,
  type Transaction,
} from "@solana/kit";
import {
  findAssociatedTokenPda,
  TOKEN_PROGRAM_ADDRESS,
} from "@solana-program/token";
import { TOKEN_2022_PROGRAM_ADDRESS } from "@solana-program/token-2022";
import { stablecoinByMint, type TokenProgramKind } from "./currencies";
import { PLATFORM_FEE } from "./env";
import {
  type ParsedDflowOrder,
  parseDflowOrder,
  ValidationError,
} from "./validate";

const PROGRAM_FOR_KIND: Record<TokenProgramKind, Address> = {
  classic: TOKEN_PROGRAM_ADDRESS,
  token2022: TOKEN_2022_PROGRAM_ADDRESS,
};

// Derive the fee ATA for the output mint, owned by the configured platform
// fee wallet. DFlow defaults to `platformFeeMode=outputMint`, so the fee
// account must hold the output token. Returns null when no fee is configured
// or when the output mint isn't in currencies.json (long-tail tokens that
// don't have a pre-created ATA).
async function platformFeeParams(
  outputMint: string,
): Promise<{ bps: number; feeAccount: string } | null> {
  if (!PLATFORM_FEE) return null;
  const stable = stablecoinByMint(outputMint);
  if (!stable) return null;
  const [feeAccount] = await findAssociatedTokenPda({
    owner: PLATFORM_FEE.wallet,
    mint: address(outputMint),
    tokenProgram: PROGRAM_FOR_KIND[stable.tokenProgram],
  });
  return { bps: PLATFORM_FEE.bps, feeAccount };
}

// DFlow's developer endpoint. No API key, rate-limited per-IP, CORS-enabled
// today. Swap path uses `/order` (the unified imperative endpoint) because it
// supports both classic SPL and Token-2022 mints — `/intent` doesn't.
const DFLOW_ORDER_URL = "https://dev-quote-api.dflow.net/order";

export type DflowSwapInput = {
  inputMint: string;
  outputMint: string;
  // Input amount in base units (already scaled by the from-mint's decimals).
  atomicAmount: bigint;
  // Either "auto" (server picks slippage) or a numeric basis-points string.
  slippageBps: string;
  userPublicKey: string;
  walletSession: WalletSession;
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

const CANCEL_PATTERN = /user reject|user cancel|denied|cancel/i;

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
  const fee = await platformFeeParams(outputMint);
  if (fee) {
    url.searchParams.set("platformFeeBps", String(fee.bps));
    url.searchParams.set("feeAccount", fee.feeAccount);
  }

  let res: Response;
  try {
    res = await fetch(url.toString());
  } catch {
    throw new DflowSwapError("Network error reaching DFlow", "network");
  }

  if (!res.ok) {
    const body = (await res.json().catch(() => null)) as OrderErrorBody | null;
    throw new DflowSwapError(
      body?.msg ?? body?.code ?? `Order failed (HTTP ${res.status})`,
      "api",
      res.status,
      body?.code,
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
    const msg = e instanceof Error ? e.message : String(e);
    throw new DflowSwapError(
      `DFlow returned an undecodable transaction: ${msg}`,
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
    const msg = e instanceof Error ? e.message : String(e);
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
const SWAP_CONFIRMATION_TIMEOUT_MS = 60_000;
const SWAP_CONFIRMATION_POLL_MS = 500;
// How many consecutive nulls (RPC has never seen the signature) we tolerate
// before erroring out instead of polling to timeout. ~5s of unknown is enough
// to distinguish a dropped tx from one that's just propagating slowly.
const MAX_UNKNOWN_POLLS = 10;

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
      if (unknownPolls >= MAX_UNKNOWN_POLLS) {
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
