"use client";

import { useEffect, useState } from "react";
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

const fetchPrice = (mint: string): Promise<number | null> => {
  const existing = inflight.get(mint);
  if (existing) return existing;
  const p = (async () => {
    try {
      const res = await fetch(`${JUP_PRICE_URL}?ids=${mint}`);
      if (!res.ok) return null;
      const data = (await res.json()) as Record<string, { usdPrice?: number }>;
      const price = data[mint]?.usdPrice;
      return typeof price === "number" ? price : null;
    } catch {
      return null;
    } finally {
      inflight.delete(mint);
    }
  })();
  inflight.set(mint, p);
  return p;
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
      cache.set(mint, { price: p, fetchedAt: Date.now() });
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
