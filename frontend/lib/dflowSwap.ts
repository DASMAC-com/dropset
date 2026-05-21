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

type OrderResponse = {
  transaction: string;
  inAmount: string;
  outAmount: string;
};

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

  const order = (await res.json()) as OrderResponse;
  if (!order.transaction) {
    throw new DflowSwapError("Order response missing transaction", "api");
  }

  const txBytes = getBase64Encoder().encode(order.transaction);
  const tx = getTransactionDecoder().decode(txBytes);

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
    inAmount: BigInt(order.inAmount),
    outAmount: BigInt(order.outAmount),
  };
}

// Wallet `sendTransaction` returns after submission, not after the chain has
// confirmed the tx — so balance re-fetches fired immediately after see stale
// data. Poll `getSignatureStatuses` until the signature reaches `confirmed`
// (or `finalized`) and bail with an error on revert or timeout.
export async function waitForSwapConfirmation(
  rpc: SolanaClientRuntime["rpc"],
  signature: Signature,
  { timeoutMs = 60_000, pollIntervalMs = 500 } = {},
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const { value } = await rpc.getSignatureStatuses([signature]).send();
    const status = value[0];
    if (status?.err) {
      throw new DflowSwapError(
        `Transaction reverted on-chain: ${JSON.stringify(status.err)}`,
        "wallet",
      );
    }
    const cs = status?.confirmationStatus;
    if (cs === "confirmed" || cs === "finalized") return;
    await new Promise((r) => setTimeout(r, pollIntervalMs));
  }
  throw new DflowSwapError("Timed out waiting for swap confirmation", "wallet");
}
