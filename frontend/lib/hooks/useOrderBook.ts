"use client";

import { type DropsetMarketView, fetchDropsetMarketView } from "@dropset/sdk";
import type { Address } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import { stablecoinDecimals, stablecoinMint } from "../data/currencies";
import { ORDER_BOOK_REFRESH_MS } from "../data/timings";
import { readGateClocks } from "../eclob/chainClock";
import { resolveEclobRoute } from "../eclob/route";
import { type PollNext, startPollChain } from "./pollChain";

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
// The loop is the shared poll chain (lib/hooks/pollChain.ts): paused while
// the tab is hidden, and guarded so a tab-refocus can't leave two chains live
// and double the RPC cadence. It self-heals through a transient RPC error
// rather than freezing.
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

    const fire = async (isLive: () => boolean): Promise<PollNext> => {
      try {
        if (market === null) {
          const route = await resolveEclobRoute(rpc, mintA, mintB);
          if (!isLive()) return "stop";
          if (!route) {
            // No market for this pair *yet* — on localnet the bootstrap may
            // not have seeded it when the page first loads. Keep polling so
            // the book appears on its own once the market exists, rather than
            // freezing until a manual refresh remounts the hook. The panel
            // stays hidden while status is "no-market".
            setState({ ...INITIAL, status: "no-market" });
            return "again";
          }
          market = route.market;
          // sell ⇒ base=pairA, quote=pairB; buy ⇒ base=pairB, quote=pairA.
          // Resolved against the canonical ordering, so this lands on the
          // market's own base/quote whichever way the user is trading it.
          base = tokenFor(route.side === "sell" ? pairA : pairB);
          quote = tokenFor(route.side === "sell" ? pairB : pairA);
        }

        // Read the clocks here rather than letting the fetch resolve the slot,
        // so the gate and the account fetch are pinned to the same
        // `finalized` commitment. One getSlot either way; the sync's own read
        // is throttled, not per-tick. See lib/eclob/chainClock.ts.
        const { nowSlot, nowUnix } = await readGateClocks(rpc);
        if (!isLive()) return "stop";

        const view = await fetchDropsetMarketView(rpc, market, {
          commitment: "finalized",
          nowSlot,
          nowUnix,
        });
        if (!isLive()) return "stop";
        setState({ status: "ready", view, market, base, quote });
      } catch {
        // A transient RPC hiccup shouldn't freeze the book — keep polling so
        // it self-heals on the next tick. The last good view stays on screen.
      }
      return "again";
    };

    const chain = startPollChain({
      intervalMs: ORDER_BOOK_REFRESH_MS,
      initialDelayMs: 0,
      fire,
    });
    return chain.stop;
  }, [pairA, pairB, enabled, client]);

  return state;
}
