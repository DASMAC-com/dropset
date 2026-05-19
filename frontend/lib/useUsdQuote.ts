"use client";

import { useEffect, useState, useSyncExternalStore } from "react";
import { stablecoinMint } from "./currencies";

// Jupiter's keyless Price API endpoint. Hit directly from the browser so each
// user's IP gets its own rate-limit bucket (1 RPS keyless) instead of every
// session funneling through our server's single IP. CORS is enabled here, and
// no API key is required.
const JUP_PRICE_URL = "https://lite-api.jup.ag/price/v3";
const CACHE_TTL_MS = 30_000;

type CacheEntry = { price: number | null; fetchedAt: number };
const cache = new Map<string, CacheEntry>();
const inflight = new Map<string, Promise<number | null>>();

// Bump on every cache mutation so React consumers wired through
// `useSyncExternalStore` re-render when prices resolve.
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

const fetchPrice = (mint: string): Promise<number | null> => {
  const existing = inflight.get(mint);
  if (existing) return existing;
  const p = (async () => {
    try {
      const res = await fetch(`${JUP_PRICE_URL}?ids=${mint}`);
      if (!res.ok) return null;
      const data = (await res.json()) as Record<string, { usdPrice?: number }>;
      const usd = data[mint]?.usdPrice;
      const price = typeof usd === "number" ? usd : null;
      cache.set(mint, { price, fetchedAt: Date.now() });
      notify();
      return price;
    } catch {
      return null;
    } finally {
      inflight.delete(mint);
    }
  })();
  inflight.set(mint, p);
  return p;
};

// Warm the cache for a list of mints in a single batched call so the swap UI
// has prices ready before the user opens the picker. Skips mints that are
// already fresh or already in flight, and dedupes parallel `useUsdQuote` calls
// onto the same network request via the inflight map.
export const prefetchAllPrices = (mints: string[]): Promise<void> => {
  const now = Date.now();
  const need = mints.filter((m) => {
    const hit = cache.get(m);
    if (hit && now - hit.fetchedAt < CACHE_TTL_MS) return false;
    return !inflight.has(m);
  });
  if (need.length === 0) return Promise.resolve();
  const batch = (async () => {
    try {
      const res = await fetch(`${JUP_PRICE_URL}?ids=${need.join(",")}`);
      if (!res.ok) return;
      const data = (await res.json()) as Record<string, { usdPrice?: number }>;
      const at = Date.now();
      for (const mint of need) {
        const usd = data[mint]?.usdPrice;
        cache.set(mint, {
          price: typeof usd === "number" ? usd : null,
          fetchedAt: at,
        });
      }
      notify();
    } catch {
      // Leave cache as-is; on-demand `fetchPrice` will retry per-mint.
    }
  })();
  for (const mint of need) {
    const p = batch.then(() => cache.get(mint)?.price ?? null);
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
    return hit && Date.now() - hit.fetchedAt < CACHE_TTL_MS ? hit.price : null;
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
      setPrice(hit.price);
      setLoading(false);
      return;
    }
    // Stale or missing — clear so we don't show the prior stablecoin's price
    // attributed to this one while the fetch is in flight.
    setPrice(null);
    setLoading(true);
    let cancelled = false;
    fetchPrice(mint).then((p) => {
      if (cancelled) return;
      setPrice(p);
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

// Returns a lookup function for the current price cache. Subscribes once at
// the component level via `useSyncExternalStore`, so the consumer re-renders
// when prices resolve. The returned function classifies each mint as:
//   - "unknown"  → no cache entry yet (prefetch hasn't completed)
//   - "illiquid" → Jupiter responded but had no usable price for this mint
//   - "liquid"   → Jupiter returned a numeric price
export const useLiquidityLookup = (): ((mint: string) => Liquidity) => {
  useSyncExternalStore(subscribe, getVersion, getVersion);
  return (mint: string) => {
    const hit = cache.get(mint);
    if (!hit) return "unknown";
    return hit.price === null ? "illiquid" : "liquid";
  };
};
