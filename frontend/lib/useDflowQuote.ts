"use client";

import { useEffect, useState } from "react";
import {
  DFLOW_QUOTE,
  markExhausted,
  projectedRemaining,
  recordResponse,
} from "./rateLimitBudget";

// DFlow developer trade API. No API key required and CORS open
// (`access-control-allow-origin: *`), so we call directly from the browser
// and each user's IP gets its own 60-token bucket (refills ~1/sec). When
// we outgrow that we'll proxy through a Next.js route handler — the hook
// signature stays the same, only the base URL changes.
const DFLOW_QUOTE_URL = "https://dev-quote-api.dflow.net/quote";

// Idle window after an input change before the fetch fires. Keeps typing
// from emitting one request per keystroke.
const DEBOUNCE_MS = 500;

// Auto-refresh cadence after a successful fetch. 2 s gives a fresh route
// view while leaving plenty of bucket headroom for typing bursts.
const REFRESH_MS = 2_000;

// Defensive floor for projected `remaining`. Drop below this and the
// timer defers another cycle rather than risk a 429.
const MIN_TOKENS_TO_FETCH = 3;

// When the budget is exhausted, hold off until projected remaining reaches
// this many tokens.
const RECOVERY_TOKEN_TARGET = 10;

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

export type DflowQuote = {
  status: QuoteStatus;
  outAmount: bigint | null;
  inAmount: bigint | null;
  priceImpactPct: string | null;
  slippageBps: number | null;
  hasQuote: boolean;
  error: string | null;
};

const INITIAL: DflowQuote = {
  status: "idle",
  outAmount: null,
  inAmount: null,
  priceImpactPct: null,
  slippageBps: null,
  hasQuote: false,
  error: null,
};

// Parse a user-entered decimal string into atomic units (BigInt). Returns
// 0n for empty / whitespace / "." / NaN inputs so the caller can use a
// single `=== 0n` check to gate the fetch.
const toAtomic = (raw: string, decimals: number): bigint => {
  const s = raw.replace(/,/g, "").trim();
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

type RawQuote = {
  inAmount: string;
  outAmount: string;
  priceImpactPct?: string;
  slippageBps?: number;
};

// React hook: returns the current DFlow quote for the given inputs. A
// single self-rescheduling timer drives the whole lifecycle:
//   - On any input change the previous timer is cancelled and a new one
//     is scheduled DEBOUNCE_MS out. Holding down a key restarts the timer.
//   - After a successful fetch the next tick is scheduled REFRESH_MS out,
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
): DflowQuote => {
  const [quote, setQuote] = useState<DflowQuote>(INITIAL);

  useEffect(() => {
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
        schedule(REFRESH_MS);
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
        schedule(REFRESH_MS);
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
          const body = (await res.json().catch(() => null)) as {
            msg?: string;
          } | null;
          // Clear outAmount so the to-side falls back to "—" rather than
          // leaving a stale number from the previous good quote. We also
          // stop the chain — the next input change retries.
          setQuote({
            ...INITIAL,
            status: "error",
            error: body?.msg ?? `HTTP ${res.status}`,
          });
          return;
        }

        const data = (await res.json()) as RawQuote;
        if (cancelled) return;
        setQuote({
          status: "ok",
          outAmount: BigInt(data.outAmount),
          inAmount: BigInt(data.inAmount),
          priceImpactPct: data.priceImpactPct ?? null,
          slippageBps: data.slippageBps ?? null,
          hasQuote: true,
          error: null,
        });
        // Successful fetch — keep the chain going.
        schedule(REFRESH_MS);
      } catch (err) {
        if (cancelled || controller.signal.aborted) return;
        const message = err instanceof Error ? err.message : "Network error";
        setQuote({ ...INITIAL, status: "error", error: message });
      }
    };

    // Initial fire is debounced, so a rapid-fire input change just resets
    // the timer rather than emitting one request per keystroke.
    schedule(DEBOUNCE_MS);

    return () => {
      cancelled = true;
      if (timer !== undefined) window.clearTimeout(timer);
      controller.abort();
    };
  }, [inputMint, outputMint, inputDecimals, inputAmountDecimal]);

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
