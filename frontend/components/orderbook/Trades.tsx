"use client";

import type { Address } from "@solana/kit";
import { ExternalLink } from "@/components/icons";
import { explorerTxUrl } from "@/lib/explorer";
import type { BookToken } from "@/lib/hooks/useOrderBook";
import type { RecentFill } from "@/lib/hooks/useRecentFills";
import { useRecentFills } from "@/lib/hooks/useRecentFills";
import {
  amountFractionDigits,
  amountValue,
  formatAmount,
  formatClockTime,
  formatPrice,
  priceFractionDigits,
} from "./format";
import { GREEN, RED } from "./tone";

const ROW_H = "h-[22px]";

// One grid template for the header and every row, so the columns line up.
// The last two tracks are fixed widths rather than `auto` on purpose: `auto`
// sizes to each grid's own content, and the header's "Time" is narrower than a
// row's "11:39:41", which silently pushed the two 1fr tracks out of step. The
// explorer link gets its own track so the header label right-aligns over the
// timestamps and not over the icon.
//
// 4.5rem holds `HH:MM:SS` at text-[11px] in the mono face with room to spare;
// 1rem holds the 10px icon plus its padding. Generous on both — the content is
// right-aligned, so any slack falls harmlessly to the left.
const COLS = "grid grid-cols-[1fr_1fr_4.5rem_1rem] items-center gap-x-4";

// Taker side → colour: a buy is green, a sell is red. (Note this is the
// opposite mapping from the ladder above, where the *levels* a buy consumes —
// the asks — are red. See ./tone.)
const sideTone = (side: RecentFill["side"]) => (side === "buy" ? GREEN : RED);

function TradeRow({
  fill,
  fractionDigits,
  decimals,
  isNewest,
}: {
  fill: RecentFill;
  fractionDigits: number;
  decimals: number;
  isNewest: boolean;
}) {
  // Size precision is a function of this row's own value, nothing else. An
  // earlier version shared one count across the pane (computed from the
  // smallest visible fill) to keep the decimal point aligned — but that count
  // changed every time a dust leg scrolled into or out of the window, so the
  // whole column visibly flipped between 2 and 8 decimals. A per-row count is
  // ragged but never moves once the row is on screen.
  const sizeFractionDigits = amountFractionDigits(
    amountValue(fill.size, decimals),
  );

  return (
    // Zebra striping: the pane's own background on odd rows, the grey `muted`
    // token on even ones, so a long tape stays readable as it scrolls.
    <div
      className={`relative ${COLS} px-3 ${ROW_H} text-[11px] tabular-nums even:bg-muted`}
    >
      {/* Arrival flash on the top row only, tinted by the taker side. Keyed on
          the fill id so React mounts a fresh node — and so replays the
          animation — each time a new trade takes the top slot. Absolute, so it
          sits outside the grid's tracks. */}
      {isNewest && (
        <div
          key={fill.id}
          className="pointer-events-none absolute inset-0"
          style={{
            backgroundColor: sideTone(fill.side),
            opacity: 0.3,
            animation: "ob-flash 0.6s ease-out forwards",
          }}
        />
      )}
      <span className="z-10 font-mono" style={{ color: sideTone(fill.side) }}>
        {formatPrice(fill.price, fractionDigits)}
      </span>
      <span className="z-10 text-right font-mono text-foreground">
        {formatAmount(fill.size, decimals, sizeFractionDigits)}
      </span>
      <span className="z-10 text-right font-mono text-muted-fg">
        {formatClockTime(fill.time)}
      </span>
      <a
        href={explorerTxUrl(fill.signature)}
        target="_blank"
        rel="noopener noreferrer"
        title="View this trade's transaction in the explorer"
        // Always visible, on every row: the link is the point of the column
        // during a demo, and a hover-reveal hides it from anyone who isn't
        // already pointing at the row they want.
        className="z-10 rounded p-0.5 text-muted-fg transition-colors hover:text-accent"
      >
        <ExternalLink size={10} />
      </a>
    </div>
  );
}

/**
 * The trades tape: the live counterpart to the resting-depth ladder. Newest
 * trade on top, oldest falling off the bottom of a fixed window.
 *
 * Each row is one fill leg — the on-chain `FillEvent` the hook decodes — so the
 * data layer keeps the protocol's "fill" vocabulary while the pane uses the
 * trader-facing "trade".
 *
 * Renders nothing until the first trade arrives — an empty pane below the book
 * would read as a broken feed rather than a quiet market, and on localnet the
 * bots may not have traded yet when the page first loads.
 */
export function Trades({
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
        <h3 className="font-semibold text-foreground text-sm">Trades</h3>
      </div>

      <div className={`${COLS} px-3 py-1 text-[10px] text-muted-fg`}>
        <span>Price</span>
        <span className="text-right">Size ({base.symbol})</span>
        <span className="text-right">Time</span>
        {/* Empty cell under the explorer-link track, so "Time" right-aligns
            over the timestamps rather than over the icons. */}
        <span />
      </div>

      {/* Rows in their own container so the zebra striping's nth-child parity
          counts trades, not the header row above them. */}
      <div>
        {fills.map((fill, index) => (
          <TradeRow
            key={fill.id}
            fill={fill}
            fractionDigits={fractionDigits}
            decimals={base.decimals}
            isNewest={index === 0}
          />
        ))}
      </div>
    </div>
  );
}
