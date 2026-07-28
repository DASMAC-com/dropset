"use client";

import type { Address } from "@solana/kit";
import { ExternalLink } from "@/components/icons";
import { explorerTxUrl } from "@/lib/explorer";
import type { BookToken } from "@/lib/hooks/useOrderBook";
import type { RecentFill } from "@/lib/hooks/useRecentFills";
import { useRecentFills } from "@/lib/hooks/useRecentFills";
import {
  formatAmount,
  formatClockTime,
  formatPrice,
  priceFractionDigits,
} from "./format";
import { GREEN, RED } from "./tone";

const ROW_H = "h-[22px]";

// Taker side → colour: a buy is green, a sell is red. (Note this is the
// opposite mapping from the ladder above, where the *levels* a buy consumes —
// the asks — are red. See ./tone.)
const sideTone = (side: RecentFill["side"]) => (side === "buy" ? GREEN : RED);

function FillRow({
  fill,
  fractionDigits,
  decimals,
}: {
  fill: RecentFill;
  fractionDigits: number;
  decimals: number;
}) {
  return (
    <div
      className={`group relative grid grid-cols-[1fr_1fr_auto] items-center gap-2 px-3 ${ROW_H} text-[11px] tabular-nums`}
    >
      <span className="font-mono" style={{ color: sideTone(fill.side) }}>
        {formatPrice(fill.price, fractionDigits)}
      </span>
      <span className="text-right font-mono text-foreground">
        {formatAmount(fill.size, decimals)}
      </span>
      <span className="flex items-center justify-end gap-1 font-mono text-muted-fg">
        {formatClockTime(fill.time)}
        <a
          href={explorerTxUrl(fill.signature)}
          target="_blank"
          rel="noopener noreferrer"
          title="View this fill's transaction in the explorer"
          // Reveal on row hover (and on keyboard focus, so the link stays
          // reachable without a pointer) to keep the tape uncluttered.
          className="rounded p-0.5 text-muted-fg opacity-0 transition-opacity hover:text-accent focus-visible:opacity-100 group-hover:opacity-100"
        >
          <ExternalLink size={10} />
        </a>
      </span>
    </div>
  );
}

/**
 * The recent-fills tape: the live counterpart to the resting-depth ladder.
 * Newest fill on top, oldest falling off the bottom of a fixed window.
 *
 * Renders nothing until the first fill arrives — an empty pane below the book
 * would read as a broken feed rather than a quiet market, and on localnet the
 * bots may not have traded yet when the page first loads.
 */
export function RecentFills({
  market,
  base,
  enabled,
}: {
  market: Address | null;
  base: BookToken;
  enabled: boolean;
}) {
  const fills = useRecentFills(market, enabled);

  const [newest] = fills;
  if (!newest) return null;

  // One digit count for the whole tape, taken from the newest fill, so the
  // price column stays aligned as rows scroll through.
  const fractionDigits = priceFractionDigits(newest.price);

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-background">
      <div className="flex items-center justify-between border-border border-b px-3 py-2.5">
        <h3 className="font-semibold text-foreground text-sm">Recent fills</h3>
      </div>

      <div className="grid grid-cols-[1fr_1fr_auto] gap-2 px-3 py-1 text-[10px] text-muted-fg uppercase tracking-wide">
        <span>Price</span>
        <span className="text-right">Size ({base.symbol})</span>
        <span className="text-right">Time</span>
      </div>

      {fills.map((fill) => (
        <FillRow
          key={fill.id}
          fill={fill}
          fractionDigits={fractionDigits}
          decimals={base.decimals}
        />
      ))}
    </div>
  );
}
