"use client";

import { quoteEclob } from "@dropset/sdk";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { QUOTE_DEBOUNCE_MS, QUOTE_REFRESH_MS } from "../data/timings";
import { resolveEclobRoute } from "../eclob/route";
import { PLATFORM_FEE } from "../env";
import { parseAmountToBase } from "../format/balance";
import { getErrorMessage } from "../guards";
import { INITIAL_QUOTE, type QuoteState } from "../quote";

// Quote the swap directly against our own market — the core-style, eCLOB-only
// route, bypassing the aggregator entirely. The SDK's `quoteEclob` does the
// work (simulating with the exact on-chain matching math, including the
// platform fee the executor will declare); this hook is the React lifecycle
// around it and the app's mint translation, which `resolveEclobRoute` here
// applies before handing the route over.
//
// Only runs while `enabled` (route mode is eCLOB). Debounce on input change,
// then re-simulate every QUOTE_REFRESH_MS so the quote tracks the maker bot
// re-quoting the book. The simulation is local (WASM) — the only network hops
// are reading the market account and the current slot.
export const useEclobQuote = (
  inputMint: string,
  outputMint: string,
  inputDecimals: number,
  inputAmountDecimal: string,
  enabled: boolean,
): QuoteState => {
  const client = useSolanaClient();
  const [quote, setQuote] = useState<QuoteState>(INITIAL_QUOTE);

  useEffect(() => {
    if (!enabled) {
      setQuote({ ...INITIAL_QUOTE, status: "skipped" });
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
        setQuote({ ...INITIAL_QUOTE, status: "skipped" });
        return;
      }

      try {
        const route = await resolveEclobRoute(rpc, inputMint, outputMint);
        if (cancelled || gen !== generation) return;
        if (!route) {
          // Terminal: no market exists for this pair. Re-simulating won't
          // conjure one, so stop the chain until the inputs change.
          setQuote({
            ...INITIAL_QUOTE,
            status: "error",
            error: "No Dropset market for this pair",
          });
          return;
        }

        // Quote with the platform fee the executor will actually declare
        // (lib/eclob/eclobSwap.ts) — same configured rate, same clamp to this
        // market's ceiling, which `quoteEclob` applies — so the displayed
        // output is what lands in the user's account rather than a pre-fee
        // figure they never receive. The simulator composes both fees exactly
        // as the engine does, so this also keeps the quote and the fill in
        // agreement to the atom.
        const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
        if (cancelled || gen !== generation) return;

        const q = await quoteEclob(rpc, {
          leg: { route },
          amount: atomic,
          nowSlot: Number(slot),
          platformFeeBps: PLATFORM_FEE ? PLATFORM_FEE.bps : 0,
        });
        if (cancelled || gen !== generation) return;
        if (!q || q.outAmount === 0n) {
          // A thin book is transient — the maker bot re-quotes it — so keep
          // the loop alive to self-heal, unlike the terminal no-market case.
          setQuote({
            ...INITIAL_QUOTE,
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
          // Publish the *clamped* rate this quote was computed with, so the
          // panel reports what the swap will charge rather than what the env
          // asks for. Zero (a market whose ceiling turns fees off) surfaces
          // as `null` — no fee applies, so there is no rate to show.
          platformFeeBps: q.platformFeeBps > 0 ? q.platformFeeBps : null,
          venue: q.venue,
          hasQuote: true,
          error: null,
        });
        schedule(QUOTE_REFRESH_MS, gen);
      } catch (e) {
        if (cancelled || gen !== generation) return;
        // An RPC hiccup is transient; keep the loop alive so a single failed
        // read doesn't freeze the quote until the user edits an input.
        setQuote({
          ...INITIAL_QUOTE,
          status: "error",
          error: getErrorMessage(e),
        });
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
