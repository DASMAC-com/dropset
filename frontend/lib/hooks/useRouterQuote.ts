"use client";

import {
  DflowError,
  type DflowErrorKind,
  NoRouteError,
  quoteBestRoute,
} from "@dropset/sdk";
import { address } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import {
  onchainMint,
  onchainTokenProgram,
  stablecoinByMint,
} from "../data/currencies";
import {
  MIN_TOKENS_TO_FETCH,
  QUOTE_DEBOUNCE_MS,
  QUOTE_REFRESH_MS,
  RECOVERY_TOKEN_TARGET,
} from "../data/timings";
import {
  DFLOW_QUOTE,
  markExhausted,
  projectedRemaining,
  recordResponse,
} from "../dflow/rateLimitBudget";
import { PROGRAM_FOR_KIND } from "../eclob/route";
import { DFLOW_QUOTE_URL, PLATFORM_FEE } from "../env";
import { parseAmountToBase } from "../format/balance";
import { getErrorMessage } from "../guards";
import { INITIAL_QUOTE, type QuoteState } from "../quote";

// The "Best route" quote: ask the SDK router to price both our own book and
// the DFlow aggregator, and report whichever wins. Replaces the app's former
// direct call to DFlow's /quote — routing, the platform-fee guard, and the
// comparison all live in `@dropset/sdk` now, so this hook is only the React
// lifecycle around it.
//
// Slippage flag sent to the aggregator. "auto" lets DFlow size it from current
// liquidity; we render the returned `slippageBps` for transparency.
const SLIPPAGE = "auto";

// The timer mirrors the eCLOB hook: debounce on input change, then refresh on
// a cadence so the quote tracks the book (and the aggregator's routes) while
// the user just looks at the panel.
export const useRouterQuote = (
  inputMint: string,
  outputMint: string,
  inputDecimals: number,
  inputAmountDecimal: string,
  // False when the eCLOB-only route is selected — this hook stays silent.
  enabled: boolean,
  // Whether a Dropset market exists for this pair. Resolved once per pair by
  // the caller, so a pair we don't list doesn't pay market-discovery reads on
  // every tick — which is the common case on mainnet today.
  eclobAvailable: boolean,
): QuoteState => {
  const client = useSolanaClient();
  const [quote, setQuote] = useState<QuoteState>(INITIAL_QUOTE);

  useEffect(() => {
    if (!enabled) {
      setQuote({ ...INITIAL_QUOTE, status: "skipped" });
      return;
    }
    let timer: number | undefined;
    const controller = new AbortController();
    let cancelled = false;
    // Only the current fire→schedule chain may reschedule; a fire superseded by
    // a tab-refocus sees a stale generation and drops its reschedule, so
    // exactly one timer stays live and the cadence can't double.
    let generation = 0;
    const rpc = client.runtime.rpc;

    const schedule = (delay: number, gen: number) => {
      if (cancelled || gen !== generation) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => void fire(gen), delay);
    };

    const fire = async (gen: number): Promise<void> => {
      if (cancelled || gen !== generation) return;

      // Pause-on-hidden: don't quote when the tab isn't visible, but keep the
      // chain alive so we resume on visibility.
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
        // Nothing useful to quote; the effect re-runs when inputs change.
        setQuote({ ...INITIAL_QUOTE, status: "skipped" });
        return;
      }

      // Respect the aggregator's rate-limit budget. Currently dormant in the
      // browser — DFlow's dev endpoint sends `x-ratelimit-*` but doesn't
      // expose them via CORS, so `projectedRemaining` returns null and we lean
      // on DFlow's own 429. The guard reactivates behind a proxy route handler.
      const projected = projectedRemaining(DFLOW_QUOTE);
      if (projected !== null && projected < MIN_TOKENS_TO_FETCH) {
        schedule(QUOTE_REFRESH_MS, gen);
        return;
      }

      setQuote((q) => ({
        ...q,
        status: q.hasQuote ? q.status : "loading",
        error: null,
      }));

      try {
        // Our own book is priced against the on-chain mints (mock mints on
        // localnet); the aggregator prices the real ones. They coincide on
        // mainnet, which is the only cluster where this path runs today —
        // localnet forces the eCLOB-only route.
        const eclobLeg =
          eclobAvailable &&
          stablecoinByMint(inputMint) &&
          stablecoinByMint(outputMint)
            ? {
                inputMint: address(onchainMint(inputMint)),
                outputMint: address(onchainMint(outputMint)),
                inputTokenProgram:
                  PROGRAM_FOR_KIND[onchainTokenProgram(inputMint)],
                outputTokenProgram:
                  PROGRAM_FOR_KIND[onchainTokenProgram(outputMint)],
              }
            : null;

        // Only our own book needs a slot (it scopes flush-level expiry), so an
        // aggregator-only tick — the common case on mainnet today — makes no
        // RPC call at all.
        let currentSlot: number | undefined;
        if (eclobLeg) {
          const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
          if (cancelled || gen !== generation) return;
          currentSlot = Number(slot);
        }

        const { best } = await quoteBestRoute(rpc, {
          amount: atomic,
          currentSlot,
          signal: controller.signal,
          eclob: eclobLeg,
          aggregator: {
            quoteUrl: DFLOW_QUOTE_URL,
            inputMint,
            outputMint,
            slippageBps: SLIPPAGE,
            // The guard: the SDK checks the fee ATA exists before declaring
            // anything, so a mint without a pre-created vault skips the fee
            // rather than breaking the route.
            platformFee: PLATFORM_FEE
              ? { ...PLATFORM_FEE, mint: address(onchainMint(outputMint)) }
              : null,
            onResponse: (res) => recordResponse(DFLOW_QUOTE, res),
          },
        });
        if (cancelled || gen !== generation) return;

        setQuote({
          status: "ok",
          outAmount: best.outAmount,
          inAmount: best.inAmount,
          inputMint,
          outputMint,
          priceImpactPct: best.venue === "dflow" ? best.priceImpactPct : null,
          slippageBps: best.venue === "dflow" ? best.slippageBps : null,
          venue: best.venue,
          hasQuote: true,
          error: null,
        });
        schedule(QUOTE_REFRESH_MS, gen);
      } catch (e) {
        if (cancelled || gen !== generation) return;
        if (e instanceof DOMException && e.name === "AbortError") return;

        // A 429 from the aggregator pauses the chain for the recovery window.
        // The router folds each leg's failure into its candidates, so this is
        // reachable only when the whole call failed.
        if (isRateLimited(e)) {
          const untilMs = Date.now() + RECOVERY_TOKEN_TARGET * 1000;
          markExhausted(DFLOW_QUOTE, untilMs);
          setQuote((q) => ({ ...q, status: "rateLimited", error: null }));
          schedule(RECOVERY_TOKEN_TARGET * 1000, gen);
          return;
        }

        setQuote({
          ...INITIAL_QUOTE,
          status: "error",
          error: describe(e),
        });
        // Stop the chain on a terminal rejection — an unroutable pair or an
        // amount the aggregator won't fill stays that way, and re-asking every
        // QUOTE_REFRESH_MS would burn the rate-limit budget for nothing. The
        // next input change restarts it. Anything else (a thin book, a
        // transient outage) can resolve on its own, so keep polling.
        if (!isTerminal(e)) schedule(QUOTE_REFRESH_MS, gen);
      }
    };

    schedule(QUOTE_DEBOUNCE_MS, generation);

    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      generation += 1;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0, generation);
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      controller.abort();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [
    inputMint,
    outputMint,
    inputDecimals,
    inputAmountDecimal,
    enabled,
    eclobAvailable,
    client,
  ]);

  return quote;
};

// A NoRouteError carries each leg's cause, so the aggregator's own error kind
// decides how the polling chain should react.
const aggregatorErrorKind = (e: unknown): DflowErrorKind | null => {
  if (!(e instanceof NoRouteError)) return null;
  const cause = e.aggregator.cause;
  return cause instanceof DflowError ? cause.kind : null;
};

const isRateLimited = (e: unknown): boolean =>
  aggregatorErrorKind(e) === "rateLimited";

// An `api` rejection is DFlow answering "no" — a pair it can't route, or a
// size it won't fill. Re-asking on the refresh cadence won't change the
// answer, so treat it as terminal unless our own book might still recover.
const isTerminal = (e: unknown): boolean =>
  aggregatorErrorKind(e) === "api" &&
  e instanceof NoRouteError &&
  e.eclob.status === "unavailable";

// Surface the aggregator's own wording when both legs failed — the panel maps
// DFlow's "Route not found" onto a friendlier message, so keep it intact
// rather than burying it in the router's combined message.
const describe = (e: unknown): string => {
  if (e instanceof NoRouteError) {
    const agg = e.aggregator.reason;
    if (agg && e.eclob.status === "unavailable") return agg;
  }
  return getErrorMessage(e);
};
