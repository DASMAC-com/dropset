"use client";

import { useSolanaClient, useWallet } from "@solana/react-hooks";
import { useCallback, useRef, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { DflowSwapError, waitForSwapConfirmation } from "../dflow/dflowSwap";
import { executeEclobSwap } from "../eclob/eclobSwap";
import { emit } from "../events";
import { parseAmountToBase } from "../format/balance";
import { percentToBps } from "../format/percent";
import { getErrorMessage } from "../guards";
import { type Slippage, useSwapStore } from "../store";
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
  const [error, setError] = useState<DflowSwapError | null>(null);

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
      // Re-check right before signing — a disconnect could have dispatched
      // during this tick (same guard as the DFlow path).
      if (wallet.status !== "connected") {
        throw new DflowSwapError("Wallet not connected", "wallet");
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
      setResult({ ...res, fromStablecoin, toStablecoin });
      setStatus("success");
      setAmount("");
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
