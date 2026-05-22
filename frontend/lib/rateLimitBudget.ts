"use client";

import { useSyncExternalStore } from "react";
import { DFLOW_BUCKET_CAPACITY, DFLOW_REFILL_PER_SEC } from "./timings";

// Tracks per-service rate-limit budgets driven by `x-ratelimit-limit` and
// `x-ratelimit-remaining` response headers. DFlow's developer endpoint uses
// a token bucket (capacity 60, refill ~1/sec, per IP) and we extrapolate
// `remaining` forward in time using that refill rate so consumers don't
// have to wait for the next response to see the budget recover.
//
// Services are namespaced by string key (e.g. "dflow-quote") so the same
// module can later track Jupiter or other rate-limited APIs under their
// own buckets without sharing state.

export type BudgetSnapshot = {
  limit: number;
  remaining: number;
  // Wall-clock time the snapshot was taken. Used by `projectedRemaining` to
  // refill the bucket forward from the last sample.
  updatedAt: number;
  // Set when a 429 lands. Polling and debounced fetches should hold off
  // until `Date.now() >= exhaustedUntil`. Null means not rate-limited.
  exhaustedUntil: number | null;
};

const state = new Map<string, BudgetSnapshot>();
const listeners = new Set<() => void>();
let version = 0;
const notify = () => {
  version++;
  for (const l of listeners) l();
};

// Internal subscribe primitive used by `useSyncExternalStore`. The version
// counter is the snapshot value — React only re-renders when it changes,
// so consumers always read fresh state via `get()` after the subscription
// fires.
const subscribe = (cb: () => void) => {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
};
const getVersion = () => version;

// Read `x-ratelimit-*` headers off a Response and update the budget. Call
// this from every fetch wrapper so the bucket reflects reality, including
// failed/4xx responses (which still consume a token on most providers).
//
// CORS caveat: browser fetch hides response headers from JS unless the
// server includes them in `Access-Control-Expose-Headers`. DFlow's dev
// endpoint sends `x-ratelimit-*` but does not currently expose them, so
// `headers.get(...)` returns null from the browser. We bail in that case
// — `Number(null)` is 0, which would otherwise look like a permanently
// exhausted bucket and break our defensive `projected < N` guards.
export const recordResponse = (service: string, res: Response): void => {
  const limitStr = res.headers.get("x-ratelimit-limit");
  const remainingStr = res.headers.get("x-ratelimit-remaining");
  if (limitStr === null || remainingStr === null) return;
  const limit = Number(limitStr);
  const remaining = Number(remainingStr);
  if (!Number.isFinite(limit) || !Number.isFinite(remaining)) return;
  const prev = state.get(service);
  const next: BudgetSnapshot = {
    limit,
    remaining,
    updatedAt: Date.now(),
    // A non-429 response means the bucket accepted us; clear any prior
    // exhaustion. The defensive `< 3` guard in the quote hook keeps us
    // from re-tripping immediately.
    exhaustedUntil: res.status === 429 ? (prev?.exhaustedUntil ?? null) : null,
  };
  state.set(service, next);
  notify();
};

// Mark the service as rate-limited until a wall-clock deadline. The quote
// hook computes the deadline from current `remaining` + refill rate so the
// UI can count down precisely.
export const markExhausted = (service: string, untilMs: number): void => {
  const prev = state.get(service);
  const base: BudgetSnapshot = prev ?? {
    limit: DFLOW_BUCKET_CAPACITY,
    remaining: 0,
    updatedAt: Date.now(),
    exhaustedUntil: null,
  };
  state.set(service, { ...base, exhaustedUntil: untilMs });
  notify();
};

// Estimate the bucket's `remaining` *right now* by refilling forward from
// the last sample at REFILL_PER_SEC. Clamped to `[0, limit]`. Returns null
// if no response has been recorded yet.
export const projectedRemaining = (
  service: string,
  now: number = Date.now(),
): number | null => {
  const s = state.get(service);
  if (!s) return null;
  const elapsedSec = Math.max(0, (now - s.updatedAt) / 1000);
  const refilled = s.remaining + elapsedSec * DFLOW_REFILL_PER_SEC;
  return Math.max(0, Math.min(s.limit, Math.floor(refilled)));
};

// Synchronous snapshot read for hook getters. Returns the raw last-sample
// snapshot; callers that need a refilled `remaining` should use
// `projectedRemaining`.
export const getBudget = (service: string): BudgetSnapshot | null =>
  state.get(service) ?? null;

// React hook returning the live budget for a service. Re-renders on every
// `recordResponse` / `markExhausted` for the same or any service — the
// version counter is shared. Callers can derive a projected remaining via
// `projectedRemaining(service)` for countdown UIs.
export const useRateLimit = (service: string): BudgetSnapshot | null => {
  useSyncExternalStore(subscribe, getVersion, () => 0);
  return getBudget(service);
};

// Public service-key constants so consumers don't drift on the string.
export const DFLOW_QUOTE = "dflow-quote";
