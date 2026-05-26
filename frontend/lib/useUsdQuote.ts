"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { stablecoinMint } from "./currencies";
import { JUPITER_SEARCH_URL } from "./env";
import { getErrorMessage } from "./guards";
import {
  JUPITER_FETCH_TIMEOUT_MS,
  TOKEN_INFO_REFRESH_MS,
  TOKEN_INFO_TTL_MS,
} from "./timings";
import { type ParsedJupiterRow, parseJupiterSearchResponse } from "./validate";

// Re-export so existing import sites (SwapPanel, CurrenciesView) keep
// working — the canonical definition lives in lib/timings.ts.
export const REFRESH_INTERVAL_MS = TOKEN_INFO_REFRESH_MS;

// Jupiter's keyless Tokens API endpoint lives in lib/env.ts
// (JUPITER_SEARCH_URL). Hit directly from the browser so each user's IP
// gets its own rate-limit bucket (1 RPS keyless) instead of every session
// funneling through our server's single IP. CORS is enabled, no API key
// required. /tokens/v2/search returns price + market data in a single
// batched call for up to 100 mints, which we use to power both the swap
// UI's USD readouts and the /currencies market-data columns.

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

const fetchInfo = (mint: string): Promise<TokenInfo | null> => {
  const existing = inflight.get(mint);
  if (existing) return existing;
  const p = (async () => {
    try {
      const res = await fetch(`${JUPITER_SEARCH_URL}?query=${mint}`, {
        signal: AbortSignal.timeout(JUPITER_FETCH_TIMEOUT_MS),
      });
      if (!res.ok) {
        console.warn(`Jupiter token info HTTP ${res.status} for ${mint}`);
        return null;
      }
      const body: unknown = await res.json();
      const rows = parseJupiterSearchResponse(body);
      if (rows === null) {
        console.warn(`Jupiter token info returned non-array for ${mint}`);
        return null;
      }
      const row = rows.find((r) => r.id === mint);
      const info = row ? project(row) : null;
      cache.set(mint, { info, fetchedAt: Date.now() });
      notify();
      return info;
    } catch (e) {
      console.warn(
        `Jupiter token info fetch failed for ${mint}: ${getErrorMessage(e)}`,
      );
      return null;
    } finally {
      inflight.delete(mint);
    }
  })();
  inflight.set(mint, p);
  return p;
};

// Warm the cache for a list of mints in a single batched call so the UI has
// prices + market data ready before the user opens the picker or browses
// /currencies. Skips mints that are already fresh or already in flight, and
// dedupes parallel per-mint fetches onto the same network request via the
// inflight map.
export const prefetchAllTokenInfo = (mints: string[]): Promise<void> => {
  const now = Date.now();
  const need = mints.filter((m) => {
    const hit = cache.get(m);
    if (hit && now - hit.fetchedAt < CACHE_TTL_MS) return false;
    return !inflight.has(m);
  });
  if (need.length === 0) return Promise.resolve();
  const batch = (async () => {
    try {
      const res = await fetch(`${JUPITER_SEARCH_URL}?query=${need.join(",")}`, {
        signal: AbortSignal.timeout(JUPITER_FETCH_TIMEOUT_MS),
      });
      if (!res.ok) {
        console.warn(
          `Jupiter prefetch HTTP ${res.status} for ${need.length} mints — per-mint fetchInfo will retry on demand`,
        );
        return;
      }
      const body: unknown = await res.json();
      const rows = parseJupiterSearchResponse(body);
      if (rows === null) {
        console.warn(
          "Jupiter prefetch returned non-array — per-mint fetchInfo will retry",
        );
        return;
      }
      const byId = new Map<string, ParsedJupiterRow>();
      for (const r of rows) byId.set(r.id, r);
      const at = Date.now();
      for (const mint of need) {
        const row = byId.get(mint);
        cache.set(mint, {
          info: row ? project(row) : null,
          fetchedAt: at,
        });
      }
      notify();
    } catch (e) {
      console.warn(
        `Jupiter prefetch failed: ${getErrorMessage(e)} — per-mint fetchInfo will retry`,
      );
    }
  })();
  for (const mint of need) {
    const p = batch.then(() => cache.get(mint)?.info ?? null);
    inflight.set(mint, p);
    p.finally(() => {
      if (inflight.get(mint) === p) inflight.delete(mint);
    });
  }
  return batch;
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
    fetchInfo(mint).then((info) => {
      if (cancelled) return;
      setPrice(info?.usdPrice ?? null);
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [mint]);

  const num = Number.parseFloat(amount.replace(/,/g, ""));
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
    if (!mint) return;
    const hit = cache.get(mint);
    if (hit && Date.now() - hit.fetchedAt < CACHE_TTL_MS) return;
    fetchInfo(mint);
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
