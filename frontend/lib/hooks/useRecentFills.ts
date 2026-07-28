"use client";

import {
  collectFillEvents,
  DROPSET_PROGRAM_ADDRESS,
  decodePrice,
} from "@dropset/sdk";
import type { Address, Signature } from "@solana/kit";
import { useSolanaClient } from "@solana/react-hooks";
import { useEffect, useState } from "react";
import {
  RECENT_FILLS_MAX_ROWS,
  RECENT_FILLS_RESUBSCRIBE_MS,
} from "../data/timings";

// One rendered row of the tape: a single fill leg.
export type RecentFill = {
  // `${signature}:${leg}` — a swap that sweeps several levels emits one
  // FillEvent per leg, so the signature alone isn't unique.
  id: string;
  signature: string;
  // Taker side. `side: 0` is an ask-side fill (the taker bought), `1` is
  // bid-side (the taker sold) — the color the row renders in.
  side: "buy" | "sell";
  // Absolute price, decoded from the event's `Price` bits.
  price: number;
  // Fill size in base atoms.
  size: bigint;
  // Unix seconds. The event carries no timestamp, so this is the
  // transaction's `blockTime`, falling back to client receipt time on the
  // rare null (a block whose time the node hasn't recorded yet).
  time: number;
};

// Cap on remembered signatures. A swap emits several fills, so this stays a
// few multiples above the row window to keep the dedup honest across a
// re-subscribe without growing without bound.
const SEEN_LIMIT = RECENT_FILLS_MAX_ROWS * 8;

const sleep = (ms: number, signal: AbortSignal): Promise<void> =>
  new Promise((resolve) => {
    const timer = setTimeout(resolve, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      resolve();
    });
  });

/**
 * Live-subscribe to the selected market's fills and keep the newest
 * {@link RECENT_FILLS_MAX_ROWS} of them, newest first.
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
 * backs off and re-subscribes rather than leaving a dead pane, and every
 * market switch tears the old chain down (the effect's abort) before the new
 * one starts, so two chains can never write to the same state.
 */
export function useRecentFills(
  market: Address | null,
  enabled: boolean,
): RecentFill[] {
  const client = useSolanaClient();
  const [fills, setFills] = useState<RecentFill[]>([]);

  useEffect(() => {
    // Drop the previous market's tape immediately. Without this a market
    // switch leaves the old pair's trades on screen until the first new fill
    // lands — which on a quiet market could be a long time.
    setFills([]);
    if (!enabled || market === null) return;

    const abort = new AbortController();
    const rpc = client.runtime.rpc;
    const rpcSubscriptions = client.runtime.rpcSubscriptions;
    // Signatures already ingested, so a re-subscribe (or a duplicate
    // notification) can't double-post a fill. Insertion-ordered, trimmed from
    // the front once it outgrows SEEN_LIMIT.
    const seen = new Set<string>();

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
      // Failures are swallowed here rather than thrown: one unreadable
      // transaction must not tear down the subscription that reads the rest.
      let tx: Awaited<ReturnType<typeof fetchTransaction>>;
      try {
        tx = await fetchTransaction(signature);
      } catch {
        return;
      }
      if (!tx || abort.signal.aborted) return;

      // `blockTime` is the fill's real timestamp; fall back to now only when
      // the node hasn't recorded one.
      const time = tx.blockTime
        ? Number(tx.blockTime)
        : Math.floor(Date.now() / 1000);

      const rows: RecentFill[] = [];
      collectFillEvents(tx).forEach((event, leg) => {
        // One program-wide subscription serves every market (logsSubscribe
        // takes a single `mentions` address), so drop the fills belonging to
        // the markets this pane isn't showing.
        if (event.market !== market) return;
        const price = decodePrice(event.fillPrice);
        if (!Number.isFinite(price) || price <= 0) return;
        rows.push({
          id: `${signature}:${leg}`,
          signature,
          side: event.side === 0 ? "buy" : "sell",
          price,
          size: event.fillBase,
          time,
        });
      });
      if (rows.length === 0 || abort.signal.aborted) return;

      // Newest transaction on top, and within it the legs stay in emission
      // order (best price first) — the order they came off the book. The legs
      // of one swap are simultaneous, so there's no newer/older among them to
      // preserve.
      setFills((prev) => [...rows, ...prev].slice(0, RECENT_FILLS_MAX_ROWS));
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
  }, [market, enabled, client]);

  return fills;
}
