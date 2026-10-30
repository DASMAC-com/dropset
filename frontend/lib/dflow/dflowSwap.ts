// cspell:word jito
"use client";

import { extractDflowApiError } from "@dropset/sdk";
import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import {
  blockhash,
  getBase64Encoder,
  getCompiledTransactionMessageDecoder,
  getTransactionDecoder,
  type SendableTransaction,
  type Signature,
  type Transaction,
} from "@solana/kit";
import { DFLOW_ORDER_TIMEOUT_MS } from "../data/timings";
import { applySlippage } from "../eclob/eclobSwap";
import { absoluteUrl, DFLOW_ORDER_URL, PLATFORM_FEE } from "../env";
import { getErrorMessage } from "../guards";
import {
  CANCEL_PATTERN,
  type Lifetime,
  SwapError,
  type SwapOutcome,
} from "../swap/types";
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

// DFlow's developer endpoint, or the keyed route handler in front of the
// production one. URL lives in lib/env.ts (DFLOW_ORDER_URL) so the choice
// stays out of this module. Swap path uses `/order` (the unified imperative
// endpoint) because it supports both classic SPL and Token-2022 mints —
// `/intent` doesn't.

export type DflowSwapInput = {
  inputMint: string;
  outputMint: string;
  // Input amount in base units (already scaled by the from-mint's decimals).
  atomicAmount: bigint;
  // Either "auto" (server picks slippage) or a numeric basis-points string.
  slippageBps: string;
  // The output the panel showed when the user pressed Swap; the order is
  // refused when it falls more than the slippage tolerance below it.
  quotedOutAmount: bigint;
  userPublicKey: string;
  walletSession: WalletSession;
  // Used to check (once, cached) whether the platform-fee vault for the output
  // mint exists on-chain before declaring the fee to DFlow.
  rpc: SolanaClientRuntime["rpc"];
  // Called once the order is fetched and checked, as it goes to the wallet.
  onSigning?: () => void;
};

// Check a decoded /order against what the user asked for, before the wallet
// is asked to sign it. The route inside the transaction is the aggregator's
// and is opaque to us — trusting it is inherent, and the endpoint is ours —
// so this is defense in depth over the parts we can check:
//   * the order spends exactly the requested input;
//   * its expected output is within the slippage tolerance of the figure the
//     panel showed (under "auto", the tolerance the server picked);
//   * the user is the fee payer and the only signer, so the wallet signs for
//     nothing but the user's own spend.
// Throws `SwapError(…, "api")` on the first mismatch. Exported to be pinned.
export const checkDflowOrder = (
  order: ParsedDflowOrder,
  tx: Transaction,
  {
    atomicAmount,
    quotedOutAmount,
    slippageBps,
    userPublicKey,
  }: Pick<
    DflowSwapInput,
    "atomicAmount" | "quotedOutAmount" | "slippageBps" | "userPublicKey"
  >,
): void => {
  if (order.inAmount !== atomicAmount) {
    throw new SwapError(
      "DFlow returned an order for a different input amount",
      "api",
    );
  }
  const tolerance =
    slippageBps === "auto" ? order.slippageBps : Number(slippageBps);
  if (order.outAmount < applySlippage(quotedOutAmount, tolerance)) {
    throw new SwapError(
      "The price moved past your slippage since the quote — review it and try again",
      "api",
    );
  }
  // The signature map is keyed by the message's signer accounts in order, the
  // fee payer first. This would reject every order if `/order`'s `sponsor`
  // parameter (another wallet paying the fee) were ever adopted — we never
  // send it, and this check must change with that decision.
  const signers = Object.keys(tx.signatures);
  if (signers.length !== 1 || signers[0] !== userPublicKey) {
    throw new SwapError(
      "DFlow returned a transaction with an unexpected fee payer or signer",
      "api",
    );
  }
};

// Execute a swap end-to-end:
//   1. GET /order with `allowAsyncExec=false` so DFlow returns a sync single
//      tx (no Jito open-order/fill split — simplest confirm path).
//   2. Base64-decode the returned transaction to a kit Transaction object, and
//      check it against the request (checkDflowOrder).
//   3. Hand it to the wallet's `sendTransaction` which signs & submits in one
//      shot. Whether it also waits for the requested `finalized` is up to
//      the wallet, so the caller's `waitForSwapConfirmation` polls for it.
export async function executeDflowSwap(
  input: DflowSwapInput,
): Promise<SwapOutcome> {
  const {
    inputMint,
    outputMint,
    atomicAmount,
    slippageBps,
    userPublicKey,
    walletSession,
    rpc,
    onSigning,
  } = input;

  const url = new URL(absoluteUrl(DFLOW_ORDER_URL));
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
    // Pinned rather than left to the server default (which is `outputMint`
    // today) so this can't drift from the quote, which pins it too — the
    // resolved feeAccount is an ATA of the output mint either way.
    url.searchParams.set("platformFeeMode", "outputMint");
  }

  const timeout = AbortSignal.timeout(DFLOW_ORDER_TIMEOUT_MS);
  let res: Response;
  try {
    res = await fetch(url.toString(), { signal: timeout });
  } catch (e) {
    if (e instanceof DOMException && e.name === "TimeoutError") {
      throw new SwapError("DFlow /order timed out — try again", "network");
    }
    throw new SwapError("Network error reaching DFlow", "network");
  }

  if (!res.ok) {
    const info = await extractDflowApiError(res);
    throw new SwapError(
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
      throw new SwapError(
        `DFlow returned an invalid order: ${e.message}`,
        "api",
        res.status,
      );
    }
    throw new SwapError(
      "DFlow order response could not be parsed",
      "api",
      res.status,
    );
  }

  let tx: Transaction;
  let lifetime: Lifetime | null;
  try {
    const txBytes = getBase64Encoder().encode(order.transaction);
    tx = getTransactionDecoder().decode(txBytes);
    // The order's expiry height, paired with the blockhash actually inside
    // its transaction, so confirmation checks the height against the chain's
    // own view of that blockhash rather than taking DFlow's figure on trust.
    // Null when the order omits the height: nothing is then ever declared
    // dropped. A durable-nonce transaction would carry a nonce here instead
    // of a blockhash; DFlow documents `lastValidBlockHeight` as the
    // blockhash's expiry, so a sync order is taken to be blockhash-lifetime.
    lifetime =
      order.lastValidBlockHeight === null
        ? null
        : {
            blockhash: blockhash(
              getCompiledTransactionMessageDecoder().decode(tx.messageBytes)
                .lifetimeToken,
            ),
            lastValidBlockHeight: order.lastValidBlockHeight,
          };
  } catch (e) {
    throw new SwapError(
      `DFlow returned an undecodable transaction: ${getErrorMessage(e)}`,
      "api",
      res.status,
    );
  }
  checkDflowOrder(order, tx, input);

  if (!walletSession.sendTransaction) {
    throw new SwapError(
      "Connected wallet doesn't support sendTransaction",
      "wallet",
    );
  }

  onSigning?.();
  let signature: Signature;
  try {
    // Cast: the DFlow tx is missing the user's signature — the wallet adds it
    // during signing. The WalletSession type asks for SendableTransaction
    // (fully signed) but at runtime Wallet Standard adapters happily complete
    // a partially-signed tx before submitting.
    signature = await walletSession.sendTransaction(
      tx as Transaction & SendableTransaction,
      { commitment: "finalized" },
    );
  } catch (e) {
    const msg = getErrorMessage(e);
    const cancelled = CANCEL_PATTERN.test(msg);
    throw new SwapError(
      cancelled ? "Cancelled in wallet" : msg,
      cancelled ? "rejected" : "wallet",
    );
  }

  return {
    signature,
    lifetime,
    inAmount: order.inAmount,
    outAmount: order.outAmount,
  };
}
