"use client";

import {
  collectFillEvents,
  DROPSET_PROGRAM_ADDRESS,
  decodePrice,
  type PriceBits,
} from "@dropset/sdk";
import type { Address, Signature } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useSyncExternalStore } from "react";
import { RECENT_FILLS_RESUBSCRIBE_MS } from "../data/timings";

// Rows held in the tape. A window, not a pad: the newest fill goes on top and
// the oldest falls off. Lives here rather than in `data/timings` — that module
// is millisecond durations, and this is a row count (the ladder keeps its own
// `MAX_ROWS` beside its pane for the same reason).
export const MAX_ROWS = 12;

// One rendered row of the tape: a single fill leg.
export type RecentFill = {
  // `${signature}:${leg}` — a swap that sweeps several levels emits one
  // FillEvent per leg, so the signature alone isn't unique.
  id: string;
  signature: string;
  // Taker side. `side: 0` is an ask-side fill (the taker bought), `1` is
  // bid-side (the taker sold) — the color the row renders in.
  side: "buy" | "sell";
  // The fill's raw on-chain `Price`, unscaled. Kept in chain-native form
  // because one tape holds every market's fills and this hook has no decimals
  // for any of them; the pane that renders a market knows its base/quote and
  // scales with `humanPrice` there.
  priceBits: PriceBits;
  // Fill size in base atoms.
  size: bigint;
  // Unix seconds. The event carries no timestamp, so this is the
  // transaction's `blockTime`, falling back to client receipt time on the
  // rare null (a block whose time the node hasn't recorded yet).
  time: number;
};

// Cap on remembered signatures. A swap emits several fills, so this stays a
// few multiples above the row window to keep the dedup honest across a
// re-subscribe without growing without bound. Shared across every market now
// that one subscription feeds them all, so it is sized against program-wide
// traffic rather than a single market's.
const SEEN_LIMIT = MAX_ROWS * 32;

const EMPTY: RecentFill[] = [];

// The tapes, module-level on purpose: they have to outlive both the hook
// instance and the selected market. Switching markets used to drop the tape on
// the floor and start the new one empty, so coming back to a market showed
// only what had traded since you returned — even though the fills had been on
// screen moments earlier, and even though the subscription below never stopped
// receiving them.
//
// Keyed by market address; each value is capped at MAX_ROWS, so the whole
// store is bounded by (markets that traded this page-load x MAX_ROWS) — every
// market the program fills, not only the ones the user opened, since one
// socket feeds them all. Page-lifetime only: a reload starts empty, because
// nothing here is persisted and history from before the socket opened is not
// recoverable from a live subscription (see the hook doc).
const tapes = new Map<Address, RecentFill[]>();

// Signatures already ingested, so a re-subscribe (or a duplicate notification)
// can't double-post a fill. Insertion-ordered, trimmed from the front once it
// outgrows SEEN_LIMIT.
const seen = new Set<string>();

// `useSyncExternalStore` subscribers — one per mounted tape pane.
const tapeListeners = new Set<() => void>();

// Prepend to a market's tape. Replaces the array rather than mutating it: the
// snapshot below hands the stored reference straight to React, which compares
// by identity to decide whether to re-render.
function prependTape(market: Address, rows: RecentFill[]): void {
  const prev = tapes.get(market) ?? EMPTY;
  tapes.set(market, [...rows, ...prev].slice(0, MAX_ROWS));
}

function notifyTapes(): void {
  tapeListeners.forEach((listener) => {
    listener();
  });
}

// Stable identity across renders, which `useSyncExternalStore` requires of its
// subscribe argument — a fresh closure per render would re-subscribe on every
// one.
function subscribeToTapes(onStoreChange: () => void): () => void {
  tapeListeners.add(onStoreChange);
  return () => {
    tapeListeners.delete(onStoreChange);
  };
}

// Abortable delay. The listener is registered `once` and removed on the timer
// path, so a long run of backoff cycles against a down validator doesn't pile
// up listeners on the effect's single long-lived signal.
const sleep = (ms: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve) => {
    const onAbort = () => {
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(() => {
      signal.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    signal.addEventListener("abort", onAbort, { once: true });
  });

/**
 * Live-subscribe to the selected market's fills and keep the newest
 * {@link MAX_ROWS} of them, newest first.
 *
 * Fills come from the chain the same way the TUI and maker-bot read them:
 * `logsSubscribe` on the program tells us a transaction touched it, then
 * `getTransaction` supplies the inner instructions the `emit_cpi!` events
 * actually live in (the logs don't carry them). The SDK's `collectFillEvents`
 * does the extraction, including verifying each event's emitting program —
 * the envelope's tag and discriminator are public, so that check is what makes
 * an event trustworthy.
 *
 * The subscription self-heals: a dropped websocket or a transient RPC error
 * backs off and re-subscribes rather than leaving a dead pane.
 *
 * It is deliberately **not** keyed on the selected market. `logsSubscribe`
 * takes a single `mentions` address, so one program-wide socket already
 * carries every market's fills; the pane only ever needed to choose which of
 * them to render. So while this hook is mounted every market's tape fills,
 * not just the visible one's — switch away and back and the tape shows what
 * traded while you were gone, rather than restarting from empty.
 *
 * Two limits on that, both worth knowing before relying on it. The tapes
 * survive because they live in the module-level store below, **not** because
 * the socket is continuous: `useOrderBook` resets to `idle` on a pair change,
 * which unmounts the whole panel and with it this hook, so a market switch
 * does still tear the socket down and re-subscribe. Fills landing inside that
 * resolve-and-first-fetch window are missed. A direction flip is the case that
 * no longer remounts, and there the socket genuinely is continuous.
 *
 * What this still cannot show is history from before the socket opened: a live
 * subscription has no past, so a fresh page load starts every tape empty and
 * fills in from the next trade. Backfilling that would mean walking
 * `getSignaturesForAddress` and decoding each transaction — the indexer's job,
 * not the pane's.
 *
 * Unlike the polling hooks beside it (`useOrderBook`, `useEclobQuote`), this one
 * does **not** pause on a hidden tab. Pausing a poll just skips a tick, but
 * pausing a push subscription means tearing down the socket and losing the
 * trades that happen while it's down — the tape would come back with a hole in
 * it. The cost of staying subscribed is the per-notification fetch below.
 */
export function useRecentFills(
  market: Address | null,
  enabled: boolean,
): RecentFill[] {
  const client = useSolanaClient();

  // Read this market's tape out of the module-level store. The snapshot is the
  // stored array itself, so it stays referentially stable until that market
  // actually gains a fill — a fill on some other market re-runs the snapshot
  // but returns the same reference, and React skips the re-render.
  const fills = useSyncExternalStore(
    subscribeToTapes,
    () => (market === null ? EMPTY : (tapes.get(market) ?? EMPTY)),
    () => EMPTY,
  );

  useEffect(() => {
    if (!enabled) return;

    const abort = new AbortController();
    const rpc = client.runtime.rpc;
    const rpcSubscriptions = client.runtime.rpcSubscriptions;

    const fetchTransaction = (signature: Signature) =>
      rpc
        .getTransaction(signature, {
          commitment: "confirmed",
          encoding: "json",
          maxSupportedTransactionVersion: 0,
        })
        .send({ abortSignal: abort.signal });

    const ingest = async (signature: Signature): Promise<void> => {
      if (seen.has(signature)) return;
      seen.add(signature);
      if (seen.size > SEEN_LIMIT) {
        const oldest = seen.values().next();
        if (!oldest.done) seen.delete(oldest.value);
      }

      // Every notification costs one getTransaction, including the maker bots'
      // reprice traffic that carries no fills — `logsSubscribe` can only filter
      // by mentioned address, and the events live in the inner instructions
      // rather than the logs, so there's nothing cheaper to pre-filter on. The
      // TUI's fill subscription pays the same cost against the same node.
      //
      // Ingest is awaited per notification, which is what keeps rows prepending
      // in arrival order. The ceiling that implies: notifications arrive for the
      // whole program, so if program-wide traffic outpaces the RPC round-trip
      // the queue grows and the tape lags behind the chain. Fine at demo scale;
      // a busier market would want the fetches pipelined and re-ordered on the
      // way out.
      //
      // Failures are swallowed here rather than thrown: one unreadable
      // transaction must not tear down the subscription that reads the rest.
      // Release the claim on any path that ends without the fill reaching a
      // tape. The claim is taken before the round-trip so two concurrent
      // chains can't both ingest one signature — but `seen` now outlives the
      // effect, so a claim abandoned mid-flight would suppress that signature
      // for the life of the page rather than just the life of the chain. The
      // pane unmounts on every market switch (see the doc above), which is
      // exactly when an in-flight fetch gets aborted, so this is reachable
      // traffic and not a theoretical race.
      let tx: Awaited<ReturnType<typeof fetchTransaction>>;
      try {
        tx = await fetchTransaction(signature);
      } catch {
        seen.delete(signature);
        return;
      }
      if (abort.signal.aborted) {
        seen.delete(signature);
        return;
      }
      // A null transaction is a node that has nothing for this signature, not
      // an interrupted read — it stays claimed, since re-fetching would return
      // null again.
      if (!tx) return;

      // `blockTime` is the fill's real timestamp; fall back to now only when
      // the node hasn't recorded one.
      const time = tx.blockTime
        ? Number(tx.blockTime)
        : Math.floor(Date.now() / 1000);

      // One program-wide subscription serves every market, so route each fill
      // to its own market's tape instead of keeping one and discarding the
      // rest. A single transaction can in principle touch more than one
      // market, hence the grouping rather than a single target.
      const byMarket = new Map<Address, RecentFill[]>();
      collectFillEvents(tx).forEach((event, leg) => {
        // Decoded only to reject the ZERO / INFINITY sentinels — the value
        // stored is the raw `Price`, scaled to human units at render time.
        const decoded = decodePrice(event.fillPrice);
        if (!Number.isFinite(decoded) || decoded <= 0) return;
        const row: RecentFill = {
          id: `${signature}:${leg}`,
          signature,
          side: event.side === 0 ? "buy" : "sell",
          priceBits: event.fillPrice,
          size: event.fillBase,
          time,
        };
        const bucket = byMarket.get(event.market);
        if (bucket) bucket.push(row);
        else byMarket.set(event.market, [row]);
      });
      if (abort.signal.aborted) {
        seen.delete(signature);
        return;
      }
      // Nothing to post — a reprice or a non-fill touch of the program. It
      // stays claimed: it was read in full, and re-reading it would find the
      // same absence.
      if (byMarket.size === 0) return;

      // Newest transaction on top, and within it the legs stay in emission
      // order (best price first) — the order they came off the book. The legs
      // of one swap are simultaneous, so there's no newer/older among them to
      // preserve.
      byMarket.forEach((rows, filled) => {
        prependTape(filled, rows);
      });
      notifyTapes();
    };

    const run = async (): Promise<void> => {
      while (!abort.signal.aborted) {
        try {
          const notifications = await rpcSubscriptions
            .logsNotifications(
              { mentions: [DROPSET_PROGRAM_ADDRESS] },
              { commitment: "confirmed" },
            )
            .subscribe({ abortSignal: abort.signal });
          for await (const notification of notifications) {
            if (abort.signal.aborted) return;
            // A failed transaction rolled back, so it emitted no fills.
            if (notification.value.err) continue;
            await ingest(notification.value.signature);
          }
        } catch {
          // Fall through to the backoff below: a websocket that dropped, an
          // RPC hiccup on getTransaction, or a validator that isn't up yet
          // (the localnet cold-start case) all resolve by re-subscribing.
        }
        if (abort.signal.aborted) return;
        await sleep(RECENT_FILLS_RESUBSCRIBE_MS, abort.signal);
      }
    };

    void run();

    return () => abort.abort();
  }, [enabled, client]);

  return fills;
}
