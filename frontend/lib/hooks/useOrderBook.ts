"use client";

import { type DropsetMarketView, fetchDropsetMarketView } from "@dropset/sdk";
import type { Address } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { ORDER_BOOK_REFRESH_MS } from "../data/timings";
import { gateNowSlot, gateNowUnix, syncChainClock } from "../eclob/chainClock";
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
  // The resolved market account. Exposed so a consumer that watches the same
  // market from a different transport (the recent-fills subscription) can
  // filter on it without re-resolving the route itself.
  market: Address | null;
  // The market's own base/quote in display terms. Orientation is a property
  // of the market, not of the direction being traded: the resolve runs against
  // the canonical pair (see below), so these are invariant under a from/to
  // flip. Null until the market resolves.
  base: BookToken | null;
  quote: BookToken | null;
};

const INITIAL: OrderBookState = {
  status: "idle",
  view: null,
  market: null,
  base: null,
  quote: null,
};

const tokenFor = (symbol: string): BookToken => ({
  symbol,
  decimals: stablecoinDecimals(symbol),
});

// Live-poll the on-chain order book for the selected pair, reading the book
// straight from the market account via the SDK (no indexer).
// `fetchDropsetMarketView` reconstructs it through the same WASM binding the
// swap simulator uses — the engine's own decode rather than a TS mirror of
// it — so the ladder rendered here and the quote shown on the swap path
// agree by construction. It instantiates that binding itself, so there is no
// init step to sequence here. Resolves the market once (both orientations,
// like the eCLOB quote route), then re-fetches the book every
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

  // The book belongs to the market, not to the direction it is being traded
  // in, so the poll is keyed on the *unordered* pair. Flipping from/to
  // resolves the same market account (the route search covers both
  // orientations) and the take-side compensation below lands on the same
  // base/quote, so a flip has nothing to re-fetch and nothing to redraw.
  //
  // Keying on the ordered pair instead made the swap-arrow button tear the
  // whole panel down: the effect re-ran, reset to "idle", and the panel
  // un-hid only after a fresh resolve plus first fetch — blanking the ladder
  // and the trades tape, and letting the swap card reflow into the column
  // they had vacated.
  const inOrder = fromStablecoin <= toStablecoin;
  const pairA = inOrder ? fromStablecoin : toStablecoin;
  const pairB = inOrder ? toStablecoin : fromStablecoin;

  useEffect(() => {
    // Clear any prior pair's book on every (re)run. Without this, a pair
    // switch leaves the previous market's ladder and symbols on screen (status
    // stays "ready") through the whole resolve + first-fetch round-trip —
    // showing the wrong pair's book beside the swap panel. Resetting hides the
    // panel until the new market's first poll lands. A direction flip is not a
    // pair switch, and by the keying above never re-runs this effect at all.
    setState(INITIAL);
    if (!enabled) return;
    let timer: number | undefined;
    let cancelled = false;
    // See useEclobQuote: only the current generation may reschedule, so a
    // poll chain superseded by a tab-refocus drops its reschedule instead of
    // running alongside the fresh one.
    let generation = 0;
    const rpc = client.runtime.rpc;
    // Named for the canonical pair, not for from/to: the resolver takes them
    // positionally and reports `side` relative to that ordering, so calling
    // them from/to here would read as the user's direction when it isn't.
    const mintA = stablecoinMint(pairA);
    const mintB = stablecoinMint(pairB);

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
          const route = await resolveEclobRoute(rpc, mintA, mintB);
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
          // sell ⇒ base=pairA, quote=pairB; buy ⇒ base=pairB, quote=pairA.
          // Resolved against the canonical ordering, so this lands on the
          // market's own base/quote whichever way the user is trading it.
          base = tokenFor(route.side === "sell" ? pairA : pairB);
          quote = tokenFor(route.side === "sell" ? pairB : pairA);
        }

        // Read the slot here rather than letting the fetch resolve it, so the
        // same slot can be priced for its block time and the account fetch is
        // pinned to the same commitment. One getSlot either way; the sync's
        // own read is throttled, not per-tick. See lib/eclob/chainClock.ts.
        //
        // The sync gets the raw slot (it needs a block that exists); the gate
        // gets it nudged forward, because a `confirmed` slot is behind the
        // head the engine judges against and would otherwise show depth that
        // is already gone — the slot-domain twin of the wall-clock margin.
        const slot = await rpc.getSlot({ commitment: "confirmed" }).send();
        if (cancelled || gen !== generation) return;
        await syncChainClock(rpc, slot);
        if (cancelled || gen !== generation) return;

        const view = await fetchDropsetMarketView(rpc, market, {
          commitment: "confirmed",
          nowSlot: gateNowSlot(slot),
          nowUnix: gateNowUnix(),
        });
        if (cancelled || gen !== generation) return;
        setState({ status: "ready", view, market, base, quote });
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
  }, [pairA, pairB, enabled, client]);

  return state;
}
