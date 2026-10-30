"use client";

import { quoteEclob } from "@dropset/sdk";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { QUOTE_DEBOUNCE_MS, QUOTE_REFRESH_MS } from "../data/timings";
import { readGateClocks } from "../eclob/chainClock";
import { resolveEclobRoute } from "../eclob/route";
import { PLATFORM_FEE } from "../env";
import { parseAmountToBase } from "../format/balance";
import { getErrorMessage } from "../guards";
import { formatAtomic, INITIAL_QUOTE, type QuoteState } from "../quote";
import { type PollNext, startPollChain } from "./pollChain";

// Quote the swap directly against our own market — the core-style, eCLOB-only
// route, bypassing the aggregator entirely. The SDK's `quoteEclob` does the
// work (simulating with the exact on-chain matching math, including the
// platform fee the executor will declare); this hook is the React lifecycle
// around it and the app's mint translation, which `resolveEclobRoute` here
// applies before handing the route over.
//
// Only runs while `enabled` (route mode is eCLOB). Debounce on input change,
// then re-simulate every QUOTE_REFRESH_MS so the quote tracks the maker bot
// re-quoting the book. The simulation is local (WASM), so the per-tick network
// hops are just the market account and the current slot. The chain-clock sync
// below can add a third — a `getBlockTime` — but it is cached and throttled to
// CLOCK_RESYNC_INTERVAL_SECS across every consumer, so it is far from every
// tick.
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
    const rpc = client.runtime.rpc;

    const fire = async (isLive: () => boolean): Promise<PollNext> => {
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
        return "stop";
      }

      try {
        const route = await resolveEclobRoute(rpc, inputMint, outputMint);
        if (!isLive()) return "stop";
        if (!route) {
          // Terminal: no market exists for this pair. Re-simulating won't
          // conjure one, so stop the chain until the inputs change.
          setQuote({
            ...INITIAL_QUOTE,
            status: "error",
            error: "No Dropset market for this pair",
          });
          return "stop";
        }

        // Quote with the platform fee the executor will actually declare
        // (lib/eclob/eclobSwap.ts) — same configured rate, same clamp to this
        // market's ceiling, which `quoteEclob` applies — so the displayed
        // output is what lands in the user's account rather than a pre-fee
        // figure they never receive. The simulator composes both fees exactly
        // as the engine does, so this also keeps the quote and the fill in
        // agreement to the atom.
        //
        // The clocks keep the wall-clock half of the gate honest against the
        // cluster — see lib/eclob/chainClock.ts. Quoting against an unchecked
        // device clock would let a skewed browser price levels the engine has
        // already expired.
        const { nowSlot, nowUnix } = await readGateClocks(rpc);
        if (!isLive()) return "stop";

        const q = await quoteEclob(rpc, {
          leg: { route },
          amount: atomic,
          nowSlot,
          nowUnix,
          platformFeeBps: PLATFORM_FEE ? PLATFORM_FEE.bps : 0,
        });
        if (!isLive()) return "stop";
        if (!q || q.outAmount === 0n) {
          // A thin book is transient — the maker bot re-quotes it — so keep
          // the loop alive to self-heal, unlike the terminal no-market case.
          setQuote({
            ...INITIAL_QUOTE,
            status: "error",
            error: "No liquidity for this size",
          });
          return "again";
        }
        if (q.inAmount !== atomic) {
          // A partial fill: the book can't absorb the whole input, and the
          // output would describe only the part it can, priced at its deepest
          // levels. Shown as an error (which keeps Swap disabled) rather than
          // a normal figure, as the best route's `classifyEclobQuote` excludes
          // partials. Transient like a thin book, so the loop stays alive.
          setQuote({
            ...INITIAL_QUOTE,
            status: "error",
            error: `The book can fill only ${formatAtomic(q.inAmount, inputDecimals)} — reduce the amount`,
          });
          return "again";
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
        return "again";
      } catch (e) {
        if (!isLive()) return "stop";
        // An RPC hiccup is transient; keep the loop alive so a single failed
        // read doesn't freeze the quote until the user edits an input.
        setQuote({
          ...INITIAL_QUOTE,
          status: "error",
          error: getErrorMessage(e),
        });
        return "again";
      }
    };

    const chain = startPollChain({
      intervalMs: QUOTE_REFRESH_MS,
      initialDelayMs: QUOTE_DEBOUNCE_MS,
      fire,
    });
    return chain.stop;
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
