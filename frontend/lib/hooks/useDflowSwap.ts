"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useCallback, useRef, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import {
  DflowSwapError,
  type DflowSwapResult,
  executeDflowSwap,
  waitForSwapConfirmation,
} from "../dflow/dflowSwap";
import { emit } from "../events";
import { parseAmountToBase } from "../format/balance";
import { percentToBps } from "../format/percent";
import { getErrorMessage } from "../guards";
import { type Slippage, useSwapStore } from "../store";

export type SwapStatus =
  | "idle"
  | "preparing" // GET /order in flight
  | "signing" // tx handed to wallet for signing + submission
  | "confirming" // signature received, polling RPC for on-chain confirmation
  | "success"
  | "error";

// Augment the bare DFlow result with the from/to symbols captured at the
// moment of execution. Pinning these here (rather than reading from the
// live store in the result banner) means flipping sides after a swap
// can't rewrite history on the success/error chrome.
export type CompletedSwap = DflowSwapResult & {
  fromStablecoin: string;
  toStablecoin: string;
};

// Flat shape (not a discriminated union) on purpose: SwapResult reads
// both `status` and `result` to render the success banner with a link to
// the explorer, and the success → idle transition (via reset()) should
// not require re-typing `result`. A strict union would also force the
// confirming → success transition to atomically swap status + result,
// which is harder to reason about than letting React set both in the
// same effect tick.
export type UseDflowSwap = {
  status: SwapStatus;
  result: CompletedSwap | null;
  error: DflowSwapError | null;
  execute: () => Promise<void>;
  reset: () => void;
};

const slippageBpsParam = (slip: Slippage): string =>
  slip.mode === "auto" ? "auto" : percentToBps(slip.percent).toString();

export function useDflowSwap(): UseDflowSwap {
  const wallet = useWallet();
  const client = useSolanaClient();
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const slippage = useSwapStore((s) => s.slippage);

  const [status, setStatus] = useState<SwapStatus>("idle");
  const [result, setResult] = useState<CompletedSwap | null>(null);
  const [error, setError] = useState<DflowSwapError | null>(null);

  // Track whether a swap is in flight so multiple clicks don't queue.
  const inFlight = useRef(false);

  const reset = useCallback(() => {
    setStatus("idle");
    setResult(null);
    setError(null);
  }, []);

  const execute = useCallback(async () => {
    if (inFlight.current) return;
    if (wallet.status !== "connected") {
      setError(new DflowSwapError("Wallet not connected", "wallet"));
      setStatus("error");
      return;
    }
    const fromMint = stablecoinMint(fromStablecoin);
    const toMint = stablecoinMint(toStablecoin);
    const fromDecimals = stablecoinDecimals(fromStablecoin);
    const atomicAmount = parseAmountToBase(amount, fromDecimals);
    if (atomicAmount === 0n) return;
    if (fromMint === toMint) return;

    inFlight.current = true;
    setError(null);
    setResult(null);
    setStatus("preparing");

    try {
      setStatus("signing");
      // Re-check wallet status right before handing off to the order +
      // signing path. The initial check above is racy: between then and
      // now we've awaited nothing, but useCallback could be re-invoked
      // after a disconnect dispatched during the same tick. Catching it
      // here keeps the failure mode "Wallet not connected" instead of a
      // confusing wallet-adapter error during sendTransaction.
      if (wallet.status !== "connected") {
        throw new DflowSwapError("Wallet not connected", "wallet");
      }
      const res = await executeDflowSwap({
        inputMint: fromMint,
        outputMint: toMint,
        atomicAmount,
        slippageBps: slippageBpsParam(slippage),
        userPublicKey: wallet.session.account.address.toString(),
        walletSession: wallet.session,
      });
      // Wallet `sendTransaction` resolves at submission — wait for on-chain
      // confirmation before declaring success, otherwise the balance refetch
      // we fire next sees stale state.
      setStatus("confirming");
      await waitForSwapConfirmation(client.runtime.rpc, res.signature);
      setResult({ ...res, fromStablecoin, toStablecoin });
      setStatus("success");
      // Tell components subscribed via useSplToken to refetch — the swap
      // mutated balances for both the from- and to-mints.
      emit("swapSucceeded");
    } catch (e) {
      const err =
        e instanceof DflowSwapError
          ? e
          : new DflowSwapError(getErrorMessage(e), "wallet");
      setError(err);
      setStatus("error");
    } finally {
      inFlight.current = false;
    }
  }, [wallet, client, fromStablecoin, toStablecoin, amount, slippage]);

  return { status, result, error, execute, reset };
}
