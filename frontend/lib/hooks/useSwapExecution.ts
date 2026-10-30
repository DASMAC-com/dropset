"use client";

import type { SolanaClientRuntime, WalletSession } from "@solana/client";
import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useCallback, useRef, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { emit } from "../events";
import { parseAmountToBase } from "../format/balance";
import { getErrorMessage } from "../guards";
import { type Slippage, useSwapStore } from "../store";
import { waitForSwapConfirmation } from "../swap/confirm";
import {
  type CompletedSwap,
  SwapError,
  type SwapOutcome,
  type SwapStatus,
  type UseSwap,
} from "../swap/types";

type Rpc = SolanaClientRuntime["rpc"];

// Everything a route needs to build and send one swap. `slippage` is the
// store's raw selection: each route resolves it its own way ("auto" means
// something different to each venue).
export type SwapRequest = {
  inputMint: string;
  outputMint: string;
  atomicAmount: bigint;
  slippage: Slippage;
  quotedOutAmount: bigint;
  userPublicKey: string;
  walletSession: WalletSession;
  rpc: Rpc;
  // The route calls this once the transaction is built, as it goes to the
  // wallet, so "preparing" covers the build and "signing" only the wallet.
  onSigning: () => void;
};

// The route-specific half of a swap. Pass a module-level constant: the
// scaffold's `execute` depends on it, so a fresh object each render would
// rebuild the callback each render.
export type SwapRoute<T extends SwapOutcome> = {
  submit: (req: SwapRequest) => Promise<T>;
  // Run after confirmation to settle the figures against the chain. Without
  // one the swap is reported filled at the figures `submit` returned.
  settle?: (
    rpc: Rpc,
    submitted: T,
  ) => Promise<{ outcome: SwapOutcome; filled: boolean }>;
};

// The execution scaffold both swap hooks share: the store reads, the
// status/result/error state, the in-flight guard, the wallet checks, the
// confirmation wait and the error mapping. A route supplies only how its
// transaction is built and sent, and how a confirmed one is settled.
export function useSwapExecution<T extends SwapOutcome>(
  route: SwapRoute<T>,
): UseSwap {
  const wallet = useWallet();
  const client = useSolanaClient();
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const slippage = useSwapStore((s) => s.slippage);

  const [status, setStatus] = useState<SwapStatus>("idle");
  const [result, setResult] = useState<CompletedSwap | null>(null);
  const [error, setError] = useState<SwapError | null>(null);

  // Track whether a swap is in flight so multiple clicks don't queue.
  const inFlight = useRef(false);

  const reset = useCallback(() => {
    setStatus("idle");
    setResult(null);
    setError(null);
  }, []);

  const execute = useCallback(
    async (quotedOutAmount: bigint) => {
      if (inFlight.current) return;
      if (wallet.status !== "connected") {
        setError(new SwapError("Wallet not connected", "wallet"));
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
        // Re-check wallet status right before handing off to the build +
        // signing path. The initial check above is racy: between then and
        // now we've awaited nothing, but useCallback could be re-invoked
        // after a disconnect dispatched during the same tick. Catching it
        // here keeps the failure mode "Wallet not connected" instead of a
        // confusing wallet-adapter error at the wallet call.
        if (wallet.status !== "connected") {
          throw new SwapError("Wallet not connected", "wallet");
        }
        const submitted = await route.submit({
          inputMint: fromMint,
          outputMint: toMint,
          atomicAmount,
          slippage,
          quotedOutAmount,
          userPublicKey: wallet.session.account.address.toString(),
          walletSession: wallet.session,
          rpc: client.runtime.rpc,
          onSigning: () => setStatus("signing"),
        });
        // The wallet call resolves at submission — wait for on-chain
        // confirmation before declaring success, otherwise the balance refetch
        // we fire next sees stale state.
        setStatus("confirming");
        await waitForSwapConfirmation(client.runtime.rpc, submitted.signature, {
          lifetime: submitted.lifetime,
        });
        const { outcome, filled } = route.settle
          ? await route.settle(client.runtime.rpc, submitted)
          : { outcome: submitted, filled: true };
        setResult({
          signature: outcome.signature,
          lifetime: outcome.lifetime,
          inAmount: outcome.inAmount,
          outAmount: outcome.outAmount,
          fromStablecoin,
          toStablecoin,
        });
        setStatus(filled ? "success" : "no-fill");
        // Clear the input only on a real fill: the from-balance just dropped
        // by `amount`, so leaving it populated invites a re-submit the user
        // likely can no longer fund. After a no-fill the balance is untouched
        // and the natural next action is to retry, so keeping the amount
        // saves the user re-typing it.
        if (filled) setAmount("");
        // Tell the shared balance cache (useAllBalances) to refetch. Emitted
        // on a no-fill too: it still spends the network fee, and on a
        // first-time swap into the output token the rent for that ATA as well
        // (a separate instruction, outside the swap's rollback).
        emit("swapSucceeded");
      } catch (e) {
        const err =
          e instanceof SwapError
            ? e
            : new SwapError(getErrorMessage(e), "wallet");
        setError(err);
        setStatus(err.kind === "unconfirmed" ? "unconfirmed" : "error");
        // The swap may have landed, so the balances on screen may be stale.
        if (err.kind === "unconfirmed") emit("swapSucceeded");
      } finally {
        inFlight.current = false;
      }
    },
    [
      route,
      wallet,
      client,
      fromStablecoin,
      toStablecoin,
      amount,
      slippage,
      setAmount,
    ],
  );

  return { status, result, error, execute, reset };
}
