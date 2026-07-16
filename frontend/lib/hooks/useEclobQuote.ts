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
    // A monotonically-increasing id for the live fire→schedule chain. Only the
    // current generation may reschedule; a fire that was superseded (e.g. by a
    // tab-refocus that starts a fresh chain) sees a stale `gen` and drops its
    // reschedule, so exactly one timer stays live and the RPC cadence can't
    // double on refocus.
    let generation = 0;
    const rpc = client.runtime.rpc;

    const schedule = (delay: number, gen: number) => {
      if (cancelled || gen !== generation) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => void fire(gen), delay);
    };

    const fire = async (gen: number): Promise<void> => {
      if (cancelled || gen !== generation) return;
      // Pause when the tab is hidden, but keep the chain alive to resume.
      if (document.visibilityState !== "visible") {
        schedule(QUOTE_REFRESH_MS, gen);
        return;
      }

      const atomic = parseAmountToBase(inputAmountDecimal, inputDecimals);
      if (
        !inputMint ||
        !outputMint ||
        inputMint === outputMint ||
        atomic === 0n
      ) {
        // Nothing to quote for this input; the effect re-runs when the inputs
        // change, so no reschedule.
        setQuote({ ...INITIAL, status: "skipped" });
        return;
      }

      try {
        const route = await resolveEclobRoute(rpc, inputMint, outputMint);
        if (cancelled || gen !== generation) return;
        if (!route) {
          // Terminal: no market exists for this pair. Re-simulating won't
          // conjure one, so stop the chain until the inputs change.
          setQuote({
            ...INITIAL,
            status: "error",
            error: "No Dropset market for this pair",
          });
          return;
        }

        const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
        if (cancelled || gen !== generation) return;

        // Idempotent; instantiates the WASM module on the first quote.
        await initSimulator();
        if (cancelled || gen !== generation) return;

        const q = simulateSwap(
          route.marketData,
          route.side,
          atomic,
          route.limitPriceBits,
          Number(slot),
        );
        if (cancelled || gen !== generation) return;
        if (q.outAmount === 0n) {
          // A thin book is transient — the maker bot re-quotes it — so keep
          // the loop alive to self-heal, unlike the terminal no-market case.
          setQuote({
            ...INITIAL,
            status: "error",
            error: "No liquidity for this size",
          });
          schedule(QUOTE_REFRESH_MS, gen);
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
        schedule(QUOTE_REFRESH_MS, gen);
      } catch (e) {
        if (cancelled || gen !== generation) return;
        // An RPC hiccup is transient; keep the loop alive so a single failed
        // read doesn't freeze the quote until the user edits an input.
        setQuote({ ...INITIAL, status: "error", error: getErrorMessage(e) });
        schedule(QUOTE_REFRESH_MS, gen);
      }
    };

    schedule(QUOTE_DEBOUNCE_MS, generation);

    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      // Supersede any in-flight fire so its reschedule is dropped, then start a
      // fresh chain. Without the generation bump an in-flight fire whose timer
      // already elapsed would reschedule alongside this one, doubling cadence.
      generation += 1;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0, generation);
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
