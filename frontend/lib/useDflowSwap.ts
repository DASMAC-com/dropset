"use client";

import { useWallet } from "@solana/react-hooks";
import { useCallback, useEffect, useRef, useState } from "react";
import { parseAmountToBase } from "./balance";
import { stablecoinDecimals, stablecoinMint } from "./currencies";
import {
  DflowSwapError,
  type DflowSwapResult,
  executeDflowSwap,
} from "./dflowSwap";
import { type Slippage, useSwapStore } from "./store";

export type SwapStatus =
  | "idle"
  | "preparing" // GET /order in flight
  | "signing" // tx handed to wallet (signs + sends + waits for confirm)
  | "success"
  | "error";

export type UseDflowSwap = {
  status: SwapStatus;
  result: DflowSwapResult | null;
  error: DflowSwapError | null;
  execute: () => Promise<void>;
  reset: () => void;
};

const slippageBpsParam = (slip: Slippage): string =>
  slip.mode === "auto" ? "auto" : Math.round(slip.percent * 100).toString();

export function useDflowSwap(): UseDflowSwap {
  const wallet = useWallet();
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const slippage = useSwapStore((s) => s.slippage);

  const [status, setStatus] = useState<SwapStatus>("idle");
  const [result, setResult] = useState<DflowSwapResult | null>(null);
  const [error, setError] = useState<DflowSwapError | null>(null);

  // Track whether a swap is in flight so multiple clicks don't queue.
  const inFlight = useRef(false);

  const reset = useCallback(() => {
    setStatus("idle");
    setResult(null);
    setError(null);
  }, []);

  // Reset back to idle whenever the user changes the swap inputs after a
  // success or error — they're starting fresh, so stale result/error chrome
  // would be misleading. The deps aren't read inside; they're change triggers,
  // hence the suppression.
  // biome-ignore lint/correctness/useExhaustiveDependencies: deps are reset triggers
  useEffect(() => {
    setResult(null);
    setError(null);
    setStatus((s) => (s === "success" || s === "error" ? "idle" : s));
  }, [fromStablecoin, toStablecoin, amount]);

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
      const res = await executeDflowSwap({
        inputMint: fromMint,
        outputMint: toMint,
        atomicAmount,
        slippageBps: slippageBpsParam(slippage),
        userPublicKey: wallet.session.account.address.toString(),
        walletSession: wallet.session,
      });
      setResult(res);
      setStatus("success");
    } catch (e) {
      const err =
        e instanceof DflowSwapError
          ? e
          : new DflowSwapError(
              e instanceof Error ? e.message : String(e),
              "wallet",
            );
      setError(err);
      setStatus("error");
    } finally {
      inFlight.current = false;
    }
  }, [wallet, fromStablecoin, toStablecoin, amount, slippage]);

  return { status, result, error, execute, reset };
}
