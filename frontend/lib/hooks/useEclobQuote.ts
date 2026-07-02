"use client";

import { initSimulator, simulateSwap } from "@dropset/sdk";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { QUOTE_DEBOUNCE_MS, QUOTE_REFRESH_MS } from "../data/timings";
import { resolveEclobRoute } from "../eclob/route";
import { parseAmountToBase } from "../format/balance";
import { getErrorMessage } from "../guards";
import type { DflowQuote } from "./useDflowQuote";

const INITIAL: DflowQuote = {
  status: "idle",
  outAmount: null,
  inAmount: null,
  inputMint: null,
  outputMint: null,
  priceImpactPct: null,
  slippageBps: null,
  hasQuote: false,
  error: null,
};

// Quote the swap directly against our on-chain market via the SDK's off-chain
// simulator (WASM `simulate_swap`), bypassing the DFlow aggregator entirely —
// the eCLOB-only route. Returns the same {@link DflowQuote} shape as
// useDflowQuote so the swap UI is agnostic to which route produced the quote.
//
// Only runs while `enabled` (route mode is eCLOB). The timer mirrors
// useDflowQuote: debounce on input change, then re-simulate every
// QUOTE_REFRESH_MS so the quote tracks the maker bot re-quoting the book. The
// simulation is local (WASM) — the only network hops are reading the market
// account and the current slot.
export const useEclobQuote = (
  inputMint: string,
  outputMint: string,
  inputDecimals: number,
  inputAmountDecimal: string,
  enabled: boolean,
): DflowQuote => {
  const client = useSolanaClient();
  const [quote, setQuote] = useState<DflowQuote>(INITIAL);

  useEffect(() => {
    if (!enabled) {
      setQuote({ ...INITIAL, status: "skipped" });
      return;
    }
    let timer: number | undefined;
    let cancelled = false;
    const rpc = client.runtime.rpc;

    const schedule = (delay: number) => {
      if (cancelled) return;
      timer = window.setTimeout(() => void fire(), delay);
    };

    const fire = async (): Promise<void> => {
      if (cancelled) return;
      // Pause when the tab is hidden, but keep the chain alive to resume.
      if (document.visibilityState !== "visible") {
        schedule(QUOTE_REFRESH_MS);
        return;
      }

      const atomic = parseAmountToBase(inputAmountDecimal, inputDecimals);
      if (
        !inputMint ||
        !outputMint ||
        inputMint === outputMint ||
        atomic === 0n
      ) {
        setQuote({ ...INITIAL, status: "skipped" });
        return;
      }

      try {
        const route = await resolveEclobRoute(rpc, inputMint, outputMint);
        if (cancelled) return;
        if (!route) {
          setQuote({
            ...INITIAL,
            status: "error",
            error: "No Dropset market for this pair",
          });
          return;
        }

        const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
        if (cancelled) return;

        // Idempotent; instantiates the WASM module on the first quote.
        await initSimulator();
        if (cancelled) return;

        const q = simulateSwap(
          route.marketData,
          route.side,
          atomic,
          route.limitPriceBits,
          Number(slot),
        );
        if (cancelled) return;
        if (q.outAmount === 0n) {
          setQuote({
            ...INITIAL,
            status: "error",
            error: "No liquidity for this size",
          });
          return;
        }

        setQuote({
          status: "ok",
          outAmount: q.outAmount,
          inAmount: q.inAmount,
          inputMint,
          outputMint,
          priceImpactPct: null,
          slippageBps: null,
          hasQuote: true,
          error: null,
        });
        schedule(QUOTE_REFRESH_MS);
      } catch (e) {
        if (cancelled) return;
        setQuote({ ...INITIAL, status: "error", error: getErrorMessage(e) });
      }
    };

    schedule(QUOTE_DEBOUNCE_MS);

    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0);
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [
    inputMint,
    outputMint,
    inputDecimals,
    inputAmountDecimal,
    enabled,
    client,
  ]);

  return quote;
};
