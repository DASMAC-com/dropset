"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { ALL_STABLECOIN_MINTS, stablecoinMint } from "../data/currencies";
import {
  JUPITER_FETCH_TIMEOUT_MS,
  TOKEN_INFO_REFRESH_MS,
  TOKEN_INFO_TICK_MAX_AGE_MS,
  TOKEN_INFO_TTL_MS,
} from "../data/timings";
import { JUPITER_SEARCH_URL } from "../env";
import { stripThousands } from "../format/input";
import { getErrorMessage } from "../guards";
import { type ParsedJupiterRow, parseJupiterSearchResponse } from "../validate";

// Re-export so existing import sites (SwapPanel, CurrenciesView) keep
// working — the canonical definition lives in lib/timings.ts.
export const REFRESH_INTERVAL_MS = TOKEN_INFO_REFRESH_MS;

// Jupiter's Tokens API endpoint lives in lib/env.ts (JUPITER_SEARCH_URL).
// Keyless, it is hit directly from the browser so each user's IP gets its own
// rate-limit bucket (1 RPS keyless) instead of every session funneling
// through our server's single IP. With a server-side key it is the app's own
// route handler, which adds the key. /tokens/v2/search returns price +
// market data in a single batched call for up to 100 mints, which we use to
// power both the swap UI's USD readouts and the /currencies market-data
// columns.

// Alias kept for grep-ability in this file; canonical name and rationale
// live in lib/timings.ts (TOKEN_INFO_TTL_MS).
const CACHE_TTL_MS = TOKEN_INFO_TTL_MS;

// Trimmed projection of Jupiter's /tokens/v2/search row. We keep only the
// fields the UI renders so the cache footprint stays small (~80 bytes/mint
// vs ~3 KB for the raw payload).
export type TokenInfo = {
  usdPrice: number;
  priceChange24h: number | null;
  volume24h: number | null;
  mcap: number | null;
  liquidity: number | null;
  holderCount: number | null;
};

// Rows arrive pre-validated from parseJupiterSearchResponse (every field
// is a normalized `number | null`), so projection is a straight extraction
// with no per-field typeof guards.
const project = (raw: ParsedJupiterRow): TokenInfo | null => {
  if (raw.usdPrice === null) return null;
  const change = raw.priceChange24h ?? raw.stats24h.priceChange;
  const { buyVolume, sellVolume } = raw.stats24h;
  const volume24h =
    buyVolume !== null && sellVolume !== null ? buyVolume + sellVolume : null;
  return {
    usdPrice: raw.usdPrice,
    priceChange24h: change,
    volume24h,
    mcap: raw.mcap,
    liquidity: raw.liquidity,
    holderCount: raw.holderCount,
  };
};

type CacheEntry = { info: TokenInfo | null; fetchedAt: number };
const cache = new Map<string, CacheEntry>();
const inflight = new Map<string, Promise<TokenInfo | null>>();

// Bump on every cache mutation so React consumers wired through
// `useSyncExternalStore` re-render when info resolves.
let cacheVersion = 0;
const listeners = new Set<() => void>();
const notify = () => {
  cacheVersion++;
  for (const l of listeners) l();
};
const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => {
    listeners.delete(l);
  };
};
const getVersion = () => cacheVersion;

const isFresh = (mint: string, maxAgeMs: number): boolean => {
  const hit = cache.get(mint);
  return hit !== undefined && Date.now() - hit.fetchedAt < maxAgeMs;
};

// /tokens/v2/search takes at most this many comma-separated mints per call.
const JUPITER_BATCH_MAX = 100;

// One batched call for `mints`. Every mint in the batch is cached, `null`
// when Jupiter has no row for it; a failed call caches nothing, so the next
// mount or refresh tick retries. Resolves whether the call succeeded.
const fetchBatch = async (mints: string[]): Promise<boolean> => {
  try {
    const res = await fetch(`${JUPITER_SEARCH_URL}?query=${mints.join(",")}`, {
      signal: AbortSignal.timeout(JUPITER_FETCH_TIMEOUT_MS),
    });
    if (!res.ok) {
      console.warn(
        `Jupiter token info HTTP ${res.status} for ${mints.length} mints`,
      );
      return false;
    }
    const body: unknown = await res.json();
    const rows = parseJupiterSearchResponse(body);
    if (rows === null) {
      console.warn("Jupiter token info returned non-array");
      return false;
    }
    const byId = new Map<string, ParsedJupiterRow>();
    for (const r of rows) byId.set(r.id, r);
    const at = Date.now();
    for (const mint of mints) {
      const row = byId.get(mint);
      cache.set(mint, { info: row ? project(row) : null, fetchedAt: at });
    }
    notify();
    return true;
  } catch (e) {
    console.warn(`Jupiter token info fetch failed: ${getErrorMessage(e)}`);
    return false;
  }
};

// Mints requested in the current tick, flushed together on the next
// microtask. A page's mount runs every component's effects in one pass —
// children before parents, so each swap row and /currencies row asks for its
// own mint before the page-level warm-up does — and the queue folds all of
// them into one call instead of one per mint.
const queued = new Map<string, (info: TokenInfo | null) => void>();

const flush = () => {
  const batch = new Map(queued);
  queued.clear();
  const mints = [...batch.keys()];
  for (let i = 0; i < mints.length; i += JUPITER_BATCH_MAX) {
    const chunk = mints.slice(i, i + JUPITER_BATCH_MAX);
    // A failed call resolves `null`, never an expired entry still cached.
    fetchBatch(chunk).then((ok) => {
      for (const mint of chunk)
        batch.get(mint)?.(ok ? (cache.get(mint)?.info ?? null) : null);
    });
  }
};

// Request one mint's info, joining a call already in flight or queued.
export const requestTokenInfo = (mint: string): Promise<TokenInfo | null> => {
  const existing = inflight.get(mint);
  if (existing) return existing;
  if (queued.size === 0) queueMicrotask(flush);
  const p = new Promise<TokenInfo | null>((resolve) =>
    queued.set(mint, resolve),
  );
  inflight.set(mint, p);
  p.then(() => {
    if (inflight.get(mint) === p) inflight.delete(mint);
  });
  return p;
};

// Warm the cache for `mints`, skipping any fetched within `maxAgeMs`. A
// page mount passes the TTL, so a cached navigation makes no request; a
// refresh tick passes the shorter tick age.
export const prefetchAllTokenInfo = async (
  mints: readonly string[],
  maxAgeMs: number = CACHE_TTL_MS,
): Promise<void> => {
  await Promise.all(
    mints.filter((m) => m && !isFresh(m, maxAgeMs)).map(requestTokenInfo),
  );
};

export type UsdQuote = { value: number | null; loading: boolean };

export function useUsdQuote(stablecoin: string, amount: string): UsdQuote {
  const mint = stablecoinMint(stablecoin);
  const [price, setPrice] = useState<number | null>(() => {
    const hit = cache.get(mint);
    if (!hit || Date.now() - hit.fetchedAt >= CACHE_TTL_MS) return null;
    return hit.info?.usdPrice ?? null;
  });
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!mint) {
      setPrice(null);
      setLoading(false);
      return;
    }
    const hit = cache.get(mint);
    if (hit && Date.now() - hit.fetchedAt < CACHE_TTL_MS) {
      setPrice(hit.info?.usdPrice ?? null);
      setLoading(false);
      return;
    }
    // Stale or missing — clear so we don't show the prior stablecoin's price
    // attributed to this one while the fetch is in flight.
    setPrice(null);
    setLoading(true);
    let cancelled = false;
    requestTokenInfo(mint).then((info) => {
      if (cancelled) return;
      setPrice(info?.usdPrice ?? null);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [mint]);

  const num = Number.parseFloat(stripThousands(amount));
  const safeAmt = Number.isFinite(num) ? num : 0;
  if (safeAmt === 0) return { value: 0, loading: false };
  if (price === null) return { value: null, loading };
  return { value: safeAmt * price, loading: false };
}

export type Liquidity = "unknown" | "illiquid" | "liquid";

// Returns a lookup function for the current cache. Subscribes once at the
// component level via `useSyncExternalStore`, so the consumer re-renders when
// info resolves. Classifies each mint as:
//   - "unknown"  → no cache entry yet (prefetch hasn't completed)
//   - "illiquid" → Jupiter responded but had no usable price for this mint
//   - "liquid"   → Jupiter returned a numeric price
export const useLiquidityLookup = (): ((mint: string) => Liquidity) => {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  return (mint: string) => {
    const hit = cache.get(mint);
    if (!hit) return "unknown";
    return hit.info === null ? "illiquid" : "liquid";
  };
};

// Single-mint accessor for pages that want the full market record. Returns
// `null` until the prefetch (or an on-demand fetch) resolves; thereafter,
// either the projected `TokenInfo` or `null` if Jupiter has no data.
export const useTokenInfo = (mint: string): TokenInfo | null => {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  useEffect(() => {
    if (!mint || isFresh(mint, CACHE_TTL_MS)) return;
    requestTokenInfo(mint);
  }, [mint]);
  return cache.get(mint)?.info ?? null;
};

// Bulk lookup variant for renders that need info for many mints in the same
// pass (sorting a list, lighting up a grid, etc.). Subscribes once at the
// component level via `useSyncExternalStore`, then returns a synchronous
// reader function that any number of mints can be queried against.
export const useInfoLookup = (): ((mint: string) => TokenInfo | null) => {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  return (mint: string) => cache.get(mint)?.info ?? null;
};

// Sort a list of stablecoins by 24 h USD volume descending. Tokens with no
// reported volume sink to the bottom and retain their input order — JS sort
// is stable in ES2019+, so the JSON ordering is preserved as the implicit
// fallback for both null-volume tokens and exact ties.
export const sortByVolumeDesc = <T extends { mint: string }>(
  list: T[],
  lookup: (mint: string) => TokenInfo | null,
): T[] =>
  list.slice().sort((a, b) => {
    const va = lookup(a.mint)?.volume24h ?? -1;
    const vb = lookup(b.mint)?.volume24h ?? -1;
    return vb - va;
  });

// Warm and keep-warm the Jupiter token-info cache for `mints`: the swap pair
// on /swap, the whole catalogue on /currencies and in an open picker; /vaults
// mounts no consumer and requests nothing. The keyless API rate-limits per IP, which a conference crowd shares, so the
// swap page never refreshes mints it isn't showing. Ticks skip while the
// tab is hidden.
export const useTokenInfoRefresh = (mints: readonly string[]): void => {
  // Keyed on the joined list so an inline array doesn't restart the timer.
  const key = mints.join(",");
  useEffect(() => {
    const list = key ? key.split(",") : [];
    prefetchAllTokenInfo(list);
    const id = window.setInterval(() => {
      if (document.visibilityState === "visible")
        prefetchAllTokenInfo(list, TOKEN_INFO_TICK_MAX_AGE_MS);
    }, TOKEN_INFO_REFRESH_MS);
    return () => window.clearInterval(id);
  }, [key]);
};

// The whole catalogue, for the consumers that list it.
export const useCatalogueInfoRefresh = (): void =>
  useTokenInfoRefresh(ALL_STABLECOIN_MINTS);
