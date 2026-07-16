"use client";

import { type DropsetMarketView, fetchDropsetMarketView } from "@dropset/sdk";
import type { Address } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { ORDER_BOOK_REFRESH_MS } from "../data/timings";
import { resolveEclobRoute } from "../eclob/route";

// One side of the pair, resolved to the market's own base/quote orientation.
export type BookToken = { symbol: string; decimals: number };

export type OrderBookState = {
  // "idle" before the first resolve lands (so the panel can stay hidden
  // rather than flash an empty book); "no-market" when no eCLOB market
  // exists for the pair on this cluster (the panel stays hidden);
  // "ready" once a book has been polled at least once.
  status: "idle" | "no-market" | "ready";
  view: DropsetMarketView | null;
  // The market's base/quote in display terms, oriented by the resolved
  // take side (a from→to sell makes `from` the base; a buy makes `to` the
  // base). Null until the market resolves.
  base: BookToken | null;
  quote: BookToken | null;
};

const INITIAL: OrderBookState = {
  status: "idle",
  view: null,
  base: null,
  quote: null,
};

const tokenFor = (symbol: string): BookToken => ({
  symbol,
  decimals: stablecoinDecimals(symbol),
});

// Live-poll the on-chain order book for the selected pair, reading the book
// straight from the market account via the SDK (no indexer) — the pure-TS
// `fetchDropsetMarketView`, not the WASM swap simulator, so this decode is
// independent of the swap-quote path. Resolves the market once (both
// orientations, like the eCLOB quote route), then re-fetches the book every
// ORDER_BOOK_REFRESH_MS so the maker bot's flashed depth appears live.
//
// The loop mirrors useEclobQuote's lifecycle: it pauses when the tab is
// hidden, self-heals through a transient RPC error rather than freezing, and
// carries a generation id so a tab-refocus can't leave two poll chains live
// and double the RPC cadence.
export function useOrderBook(
  fromStablecoin: string,
  toStablecoin: string,
  enabled: boolean,
): OrderBookState {
  const client = useSolanaClient();
  const [state, setState] = useState<OrderBookState>(INITIAL);

  useEffect(() => {
    // Clear any prior pair's book on every (re)run. Without this, a pair
    // switch leaves the previous market's ladder and symbols on screen (status
    // stays "ready") through the whole resolve + first-fetch round-trip —
    // showing the wrong pair's book beside the swap panel. Resetting hides the
    // panel until the new market's first poll lands.
    setState(INITIAL);
    if (!enabled) return;
    let timer: number | undefined;
    let cancelled = false;
    // See useEclobQuote: only the current generation may reschedule, so a
    // poll chain superseded by a tab-refocus drops its reschedule instead of
    // running alongside the fresh one.
    let generation = 0;
    const rpc = client.runtime.rpc;
    const fromMint = stablecoinMint(fromStablecoin);
    const toMint = stablecoinMint(toStablecoin);

    // Resolved once the market is found: its address plus the base/quote
    // tokens oriented by the take side. Cached so each poll tick is a single
    // account fetch, not a re-resolution of both orientations.
    let market: Address | null = null;
    let base: BookToken | null = null;
    let quote: BookToken | null = null;

    const schedule = (delay: number, gen: number) => {
      if (cancelled || gen !== generation) return;
      if (timer !== undefined) window.clearTimeout(timer);
      timer = window.setTimeout(() => void fire(gen), delay);
    };

    const fire = async (gen: number): Promise<void> => {
      if (cancelled || gen !== generation) return;
      // Pause when the tab is hidden, but keep the chain alive to resume.
      if (document.visibilityState !== "visible") {
        schedule(ORDER_BOOK_REFRESH_MS, gen);
        return;
      }

      try {
        if (market === null) {
          const route = await resolveEclobRoute(rpc, fromMint, toMint);
          if (cancelled || gen !== generation) return;
          if (!route) {
            // No market for this pair *yet* — on localnet the bootstrap may
            // not have seeded it when the page first loads. Keep polling so
            // the book appears on its own once the market exists, rather than
            // freezing until a manual refresh remounts the hook. The panel
            // stays hidden while status is "no-market".
            setState({ ...INITIAL, status: "no-market" });
            schedule(ORDER_BOOK_REFRESH_MS, gen);
            return;
          }
          market = route.market;
          // sell ⇒ base=from, quote=to; buy ⇒ base=to, quote=from.
          base = tokenFor(
            route.side === "sell" ? fromStablecoin : toStablecoin,
          );
          quote = tokenFor(
            route.side === "sell" ? toStablecoin : fromStablecoin,
          );
        }

        const view = await fetchDropsetMarketView(rpc, market, {
          commitment: "confirmed",
        });
        if (cancelled || gen !== generation) return;
        setState({ status: "ready", view, base, quote });
        schedule(ORDER_BOOK_REFRESH_MS, gen);
      } catch {
        // A transient RPC hiccup shouldn't freeze the book — keep polling so
        // it self-heals on the next tick. The last good view stays on screen.
        if (cancelled || gen !== generation) return;
        schedule(ORDER_BOOK_REFRESH_MS, gen);
      }
    };

    schedule(0, generation);

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
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [fromStablecoin, toStablecoin, enabled, client]);

  return state;
}
