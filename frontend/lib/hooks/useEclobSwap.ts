"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useCallback, useRef, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { executeEclobSwap } from "../eclob/eclobSwap";
import { emit } from "../events";
import { parseAmountToBase } from "../format/balance";
import { percentToBps } from "../format/percent";
import { getErrorMessage } from "../guards";
import { type Slippage, useSwapStore } from "../store";
import { waitForSwapConfirmation } from "../swap/confirm";
import { readRealizedFill } from "../swap/realizedFill";
import { SwapError } from "../swap/types";
import type { CompletedSwap, SwapStatus, UseDflowSwap } from "./useDflowSwap";

// eCLOB has no server-side "auto" slippage sizing (that's a DFlow feature), so
// an "auto" selection maps to a fixed default floor for the on-chain minOut.
const DEFAULT_ECLOB_SLIPPAGE_BPS = 50; // 0.5%

const resolveSlippageBps = (slip: Slippage): number =>
  slip.mode === "auto"
    ? DEFAULT_ECLOB_SLIPPAGE_BPS
    : percentToBps(slip.percent);

// The eCLOB-only counterpart to useDflowSwap: same status/result/error surface
// (so SwapPanel can pick either by route mode), but the swap is built and
// simulated directly against our market via executeEclobSwap — no DFlow.
export function useEclobSwap(): UseDflowSwap {
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

  const inFlight = useRef(false);

  const reset = useCallback(() => {
    setStatus("idle");
    setResult(null);
    setError(null);
  }, []);

  const execute = useCallback(async () => {
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
      setStatus("signing");
      // Re-check right before signing — a disconnect could have dispatched
      // during this tick (same guard as the DFlow path).
      if (wallet.status !== "connected") {
        throw new SwapError("Wallet not connected", "wallet");
      }
      const res = await executeEclobSwap({
        inputMint: fromMint,
        outputMint: toMint,
        atomicAmount,
        slippageBps: resolveSlippageBps(slippage),
        userPublicKey: wallet.session.account.address.toString(),
        walletSession: wallet.session,
        rpc: client.runtime.rpc,
      });
      setStatus("confirming");
      await waitForSwapConfirmation(client.runtime.rpc, res.signature);

      // Confirmation only proves the transaction didn't revert, and on our own
      // program that is not the same as the swap having happened: a fill below
      // `minOut` soft-reverts, moves nothing, and still returns Ok. So settle
      // the quoted figures against the receipt before reporting anything. A
      // null reading is "couldn't tell" rather than "didn't fill" — an
      // unreadable receipt keeps the quoted figures rather than announcing a
      // swap that did not happen.
      const realized = await readRealizedFill(
        client.runtime.rpc,
        res.signature,
        res.settlement,
      );
      // An unreadable receipt is reported as a success at the quoted figures —
      // which is exactly the pre-fix behavior, so it is worth stating why it
      // is still the right direction rather than a hole in the fix.
      //
      // The two errors are not symmetric. Announcing "no funds were swapped"
      // about a swap that did happen invites the user to swap again, at their
      // own expense. Announcing a success about a swap that did not happen is
      // self-correcting: the balance refetch fired below shows the unchanged
      // balance moments later. So when the receipt cannot be read, this fails
      // in the direction that cannot cost the user money — and `no-fill` is
      // asserted only when the chain actually said so.
      const treatAsFilled = realized === null || realized.filled;
      setResult({
        signature: res.signature,
        inAmount: realized?.amounts?.inAmount ?? res.inAmount,
        outAmount: realized?.amounts?.outAmount ?? res.outAmount,
        fromStablecoin,
        toStablecoin,
      });
      setStatus(treatAsFilled ? "success" : "no-fill");
      // Clear the input only on a real fill. After a no-fill the balance is
      // untouched and the natural next action is to retry, so keeping the
      // amount saves the user re-typing it.
      if (treatAsFilled) setAmount("");
      // Emitted either way: a no-fill still spends the network fee, and on a
      // first-time swap into the output token the rent for that ATA as well
      // (a separate instruction, outside the swap's rollback). So the balances
      // on screen are stale in both cases.
      emit("swapSucceeded");
    } catch (e) {
      const err =
        e instanceof SwapError
          ? e
          : new SwapError(getErrorMessage(e), "wallet");
      setError(err);
      setStatus("error");
    } finally {
      inFlight.current = false;
    }
  }, [
    wallet,
    client,
    fromStablecoin,
    toStablecoin,
    amount,
    slippage,
    setAmount,
  ]);

  return { status, result, error, execute, reset };
}
