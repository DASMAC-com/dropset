"use client";

import { useEffect, useState } from "react";
import {
  MIN_TOKENS_TO_FETCH,
  QUOTE_DEBOUNCE_MS,
  QUOTE_REFRESH_MS,
  RECOVERY_TOKEN_TARGET,
} from "../data/timings";
import { extractDflowApiError } from "../dflow/dflowSwap";
import {
  DFLOW_QUOTE,
  markExhausted,
  projectedRemaining,
  recordResponse,
} from "../dflow/rateLimitBudget";
import { DFLOW_QUOTE_URL } from "../env";
import { stripThousands } from "../format/input";
import {
  type ParsedDflowQuote,
  parseDflowQuote,
  ValidationError,
} from "../validate";

// DFlow developer trade API URL lives in lib/env.ts (DFLOW_QUOTE_URL). No
// API key required and CORS open today (`access-control-allow-origin: *`),
// so we call directly from the browser and each user's IP gets its own
// 60-token bucket. When we outgrow that, override
// NEXT_PUBLIC_DFLOW_QUOTE_URL to point at a proxy route handler — this
// hook stays unchanged.

// Slippage flag sent to the API. "auto" lets DFlow size it from current
// liquidity; we render the returned `slippageBps` for transparency.
const SLIPPAGE = "auto";

export type QuoteStatus =
  | "idle"
  | "loading"
  | "ok"
  | "error"
  | "rateLimited"
  | "skipped";

// Flat shape (not a discriminated union) on purpose: outAmount/inAmount
// persist across status transitions so the to-side keeps the previous
// quote visible while a new fetch is in flight. A strict
// { status: "loading"; outAmount: null } | { status: "ok"; outAmount: bigint }
// union would force consumers to either hide the prior amount during
// every refetch (flicker) or duplicate it onto the loading variant
// (bookkeeping). The mint-pair fields plus `hasQuote` are what consumers
// use to detect a stale-but-still-shown previous result.
export type DflowQuote = {
  status: QuoteStatus;
  outAmount: bigint | null;
  inAmount: bigint | null;
  // The mints this quote was fetched for. Consumers should compare these
  // against the *current* store mints to detect a stale cached quote.
  inputMint: string | null;
  outputMint: string | null;
  priceImpactPct: string | null;
  slippageBps: number | null;
  hasQuote: boolean;
  error: string | null;
};

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

// Parse a user-entered decimal string into atomic units (BigInt). Returns
// 0n for empty / whitespace / "." / NaN inputs so the caller can use a
// single `=== 0n` check to gate the fetch.
const toAtomic = (raw: string, decimals: number): bigint => {
  const s = stripThousands(raw).trim();
  if (!s || s === ".") return 0n;
  const [intPart = "0", fracRaw = ""] = s.split(".");
  if (!/^\d*$/.test(intPart) || !/^\d*$/.test(fracRaw)) return 0n;
  const frac = (fracRaw + "0".repeat(decimals)).slice(0, decimals);
  try {
    return BigInt((intPart || "0") + frac);
  } catch {
    return 0n;
  }
};

// React hook: returns the current DFlow quote for the given inputs. A
// single self-rescheduling timer drives the whole lifecycle:
//   - On any input change the previous timer is cancelled and a new one
//     is scheduled QUOTE_DEBOUNCE_MS out. Holding down a key restarts the timer.
//   - After a successful fetch the next tick is scheduled QUOTE_REFRESH_MS out,
//     keeping the quote fresh while the user just stares at the panel.
//   - 429s pause the chain for RECOVERY_TOKEN_TARGET seconds.
//   - 4xx errors and zero-amount/sameToken/missing-mint inputs stop the
//     chain entirely; the next input change is what restarts it.
//   - Cleanup aborts any in-flight fetch (via AbortController) and clears
//     the pending timer, so React Compiler can't get tripped up by
//     stale closures or side effects in updaters.
export const useDflowQuote = (
  inputMint: string,
  outputMint: string,
  inputDecimals: number,
  inputAmountDecimal: string,
  // When false the hook makes no network request and reports "skipped" — the
  // eCLOB route is active, so the DFlow aggregator must stay silent (on
  // localnet there is deliberately no call to dev-quote-api.dflow.net).
  enabled: boolean,
): DflowQuote => {
  const [quote, setQuote] = useState<DflowQuote>(INITIAL);

  useEffect(() => {
    if (!enabled) {
      setQuote({ ...INITIAL, status: "skipped" });
      return;
    }
    let timer: number | undefined;
    const controller = new AbortController();
    let cancelled = false;

    const schedule = (delay: number) => {
      if (cancelled) return;
      timer = window.setTimeout(() => {
        void fire();
      }, delay);
    };

    const fire = async (): Promise<void> => {
      if (cancelled) return;

      // Pause-on-hidden: don't fetch when the tab isn't visible, but keep
      // the chain alive so we resume on visibility. Re-checked each tick.
      if (document.visibilityState !== "visible") {
        schedule(QUOTE_REFRESH_MS);
        return;
      }

      const atomic = toAtomic(inputAmountDecimal, inputDecimals);

      // Skip conditions: there's nothing useful to fetch. Set "skipped"
      // and stop the chain — the next input change re-runs the effect
      // from scratch and reschedules.
      if (
        !inputMint ||
        !outputMint ||
        inputMint === outputMint ||
        atomic === 0n
      ) {
        setQuote({ ...INITIAL, status: "skipped" });
        return;
      }

      // Respect the budget. If projected remaining is below the floor,
      // defer one cycle rather than risk a 429.
      //
      // Currently dormant in the browser: DFlow's dev endpoint sends
      // `x-ratelimit-*` but doesn't include `Access-Control-Expose-Headers`,
      // so `recordResponse` can't read them and `projectedRemaining`
      // always returns null. The guard reactivates the moment we proxy
      // through a Next.js route handler that forwards the headers
      // explicitly. Until then we rely on DFlow itself returning 429.
      const projected = projectedRemaining(DFLOW_QUOTE);
      if (projected !== null && projected < MIN_TOKENS_TO_FETCH) {
        schedule(QUOTE_REFRESH_MS);
        return;
      }

      setQuote((q) => ({
        ...q,
        status: q.hasQuote ? q.status : "loading",
        error: null,
      }));

      try {
        const url = new URL(DFLOW_QUOTE_URL);
        url.searchParams.set("inputMint", inputMint);
        url.searchParams.set("outputMint", outputMint);
        url.searchParams.set("amount", atomic.toString());
        url.searchParams.set("slippageBps", SLIPPAGE);
        const res = await fetch(url.toString(), { signal: controller.signal });
        if (cancelled) return;
        recordResponse(DFLOW_QUOTE, res);

        if (res.status === 429) {
          const untilMs = Date.now() + RECOVERY_TOKEN_TARGET * 1000;
          markExhausted(DFLOW_QUOTE, untilMs);
          setQuote((q) => ({ ...q, status: "rateLimited", error: null }));
          schedule(RECOVERY_TOKEN_TARGET * 1000);
          return;
        }

        if (!res.ok) {
          // Shared with dflowSwap.ts — distinguish JSON error envelope from
          // malformed body so the user can tell an API error apart from an
          // upstream outage.
          const info = await extractDflowApiError(res);
          if (cancelled) return;
          setQuote({ ...INITIAL, status: "error", error: info.message });
          return;
        }

        let parsed: ParsedDflowQuote;
        try {
          const data: unknown = await res.json();
          parsed = parseDflowQuote(data);
        } catch (e) {
          if (cancelled) return;
          const reason =
            e instanceof ValidationError
              ? `Invalid quote response: ${e.message}`
              : "Quote response could not be parsed";
          setQuote({ ...INITIAL, status: "error", error: reason });
          return;
        }
        if (cancelled) return;
        setQuote({
          status: "ok",
          outAmount: parsed.outAmount,
          inAmount: parsed.inAmount,
          inputMint,
          outputMint,
          priceImpactPct: parsed.priceImpactPct,
          slippageBps: parsed.slippageBps,
          hasQuote: true,
          error: null,
        });
        // Successful fetch — keep the chain going.
        schedule(QUOTE_REFRESH_MS);
      } catch (err) {
        if (cancelled || controller.signal.aborted) return;
        // Distinguish AbortError (timeout/visibility) from real fetch
        // failures (offline, DNS, CORS) so the user can tell why.
        let message: string;
        if (err instanceof Error) {
          if (err.name === "AbortError") return;
          message =
            err.name === "TypeError"
              ? `Network unreachable: ${err.message}`
              : err.message;
        } else {
          message = "Network error";
        }
        setQuote({ ...INITIAL, status: "error", error: message });
      }
    };

    // Initial fire is debounced, so a rapid-fire input change just resets
    // the timer rather than emitting one request per keystroke.
    schedule(QUOTE_DEBOUNCE_MS);

    // Resume immediately on tab focus rather than waiting for the next
    // scheduled tick (which could be up to QUOTE_REFRESH_MS away).
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      if (timer !== undefined) window.clearTimeout(timer);
      schedule(0);
    };
    document.addEventListener("visibilitychange", onVisible);

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      controller.abort();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [inputMint, outputMint, inputDecimals, inputAmountDecimal, enabled]);

  return quote;
};

// Format an atomic BigInt amount back to a decimal string. Trailing zeros
// after the decimal point are trimmed; integer-only values render without
// a trailing dot.
export const formatAtomic = (n: bigint, decimals: number): string => {
  if (decimals === 0) return n.toString();
  const s = n.toString().padStart(decimals + 1, "0");
  const i = s.length - decimals;
  const intPart = s.slice(0, i);
  const fracPart = s.slice(i).replace(/0+$/, "");
  return fracPart ? `${intPart}.${fracPart}` : intPart;
};
