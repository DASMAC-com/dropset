"use client";

import { type BookLevel, decodePrice } from "@dropset/sdk";
import { useMemo } from "react";
import type { BookToken, OrderBookState } from "@/lib/hooks/useOrderBook";
import { useOrderBook } from "@/lib/hooks/useOrderBook";
import { useSwapStore } from "@/lib/store";

// Rows per side. Matches the protocol's per-side ladder depth (N_LEVELS = 8).
// Sides are padded to this length with empty spacer rows so the panel keeps a
// constant height as the book fills and empties — the empty→flashed moment
// reads as levels appearing in place, not the panel resizing.
const MAX_ROWS = 8;
const ROW_H = "h-[22px]";

// Hyperliquid-style soft red/green. Text is the saturated tone; the depth bar
// and the update-flash are low-alpha washes of it.
const TONE = {
  ask: {
    text: "#ff6b81",
    bar: "rgba(255,107,129,0.12)",
    flash: "rgba(255,107,129,0.30)",
  },
  bid: {
    text: "#3fd39b",
    bar: "rgba(63,211,155,0.12)",
    flash: "rgba(63,211,155,0.30)",
  },
} as const;

// One rendered ladder row: absolute price, this level's size, and the
// cumulative size from the spread out to this level (the "Total" column).
type Row = { price: number; size: bigint; total: bigint };

// FX stablecoin pairs span a wide price range (EUR ≈ 1.1, MXN ≈ 0.05,
// IDR ≈ 0.00006), so pick the fraction digits from the price magnitude and
// apply the same count to every row, keeping the price column aligned.
function priceFractionDigits(price: number): number {
  if (price >= 1000) return 2;
  if (price >= 1) return 4;
  if (price >= 0.01) return 6;
  return 8;
}

function formatPrice(price: number, fractionDigits: number): string {
  return price.toLocaleString("en-US", {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  });
}

// Compact 2-dp size, like Hyperliquid's size/total columns. Demo sizes are
// small, so the Number conversion is well inside f64's exact-integer range.
function formatAmount(atoms: bigint, decimals: number): string {
  const value = Number(atoms) / 10 ** decimals;
  return value.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
}

// Best-first levels → rows with a running cumulative total (from the spread
// outward). Drops anything that doesn't decode to a real, positive price.
function levelsToRows(levels: readonly BookLevel[]): Row[] {
  const rows: Row[] = [];
  let acc = 0n;
  for (const l of levels) {
    const price = decodePrice(l.price);
    if (!Number.isFinite(price) || price <= 0) continue;
    acc += l.size;
    rows.push({ price, size: l.size, total: acc });
    if (rows.length >= MAX_ROWS) break;
  }
  return rows;
}

function LevelRow({
  row,
  side,
  barPct,
  fractionDigits,
  decimals,
}: {
  row: Row | null;
  side: "ask" | "bid";
  barPct: number;
  fractionDigits: number;
  decimals: number;
}) {
  // Empty padding slot: a plain spacer that holds the row height. No fill, so
  // the unfilled part of the ladder stays blank instead of a solid block.
  if (!row) return <div className={ROW_H} />;

  const tone = TONE[side];
  return (
    <div
      className={`relative grid grid-cols-3 items-center px-3 ${ROW_H} text-[11px] tabular-nums`}
    >
      {/* Update flash: remounts (keyed on size) when the level changes, so a
          freshly-flashed or re-quoted level blinks, then fades. */}
      <div
        key={row.size.toString()}
        className="pointer-events-none absolute inset-0"
        style={{
          backgroundColor: tone.flash,
          animation: "ob-flash 0.6s ease-out forwards",
        }}
      />
      {/* Depth bar ∝ cumulative total, left-anchored (both sides), like HL. */}
      <div
        className="pointer-events-none absolute inset-y-0 left-0"
        style={{
          width: `${barPct}%`,
          backgroundColor: tone.bar,
          transition: "width 300ms ease-out",
        }}
      />
      <span className="z-10 font-mono" style={{ color: tone.text }}>
        {formatPrice(row.price, fractionDigits)}
      </span>
      <span className="z-10 text-right font-mono text-foreground">
        {formatAmount(row.size, decimals)}
      </span>
      <span className="z-10 text-right font-mono text-muted-fg">
        {formatAmount(row.total, decimals)}
      </span>
    </div>
  );
}

// Presentational book: asks on top (worst→best, best ask touching the
// spread), a spread row, then bids below (best→worst, best bid touching the
// spread). Depth bars share one cumulative-total scale across both sides.
function OrderBookView({
  view,
  base,
  quote,
}: {
  view: OrderBookState["view"];
  base: BookToken;
  quote: BookToken;
}) {
  const { askRows, bidRows, maxTotal, fractionDigits, spread, spreadPct } =
    useMemo(() => {
      // restingLevels is best-first: asks ascending (cheapest first), bids
      // descending (highest first). Cumulative totals accumulate from the
      // spread outward on each side.
      const asks = levelsToRows(view?.asks ?? []);
      const bids = levelsToRows(view?.bids ?? []);
      const deepestAsk = asks.at(-1)?.total ?? 0n;
      const deepestBid = bids.at(-1)?.total ?? 0n;
      const maxTotal = deepestAsk > deepestBid ? deepestAsk : deepestBid || 1n;

      const bestAsk = asks[0]?.price ?? null;
      const bestBid = bids[0]?.price ?? null;
      const fractionDigits = priceFractionDigits(bestAsk ?? bestBid ?? 1);
      const spread =
        bestAsk !== null && bestBid !== null ? bestAsk - bestBid : null;
      const mid =
        bestAsk !== null && bestBid !== null ? (bestAsk + bestBid) / 2 : null;
      const spreadPct = spread !== null && mid ? (spread / mid) * 100 : null;

      // Asks render worst→best top-to-bottom, so reverse the best-first list.
      return {
        askRows: [...asks].reverse(),
        bidRows: bids,
        maxTotal,
        fractionDigits,
        spread,
        spreadPct,
      };
    }, [view]);

  const barPct = (total: bigint) => Number((total * 100n) / maxTotal);

  // No padding: render exactly the resting levels so the panel shrinks to the
  // book rather than trailing blank rows. Keyed by price (stable per level,
  // not the array index) so React reconciles rows across polls.
  const askSlots = askRows.map((row) => ({ id: `ask-${row.price}`, row }));
  const bidSlots = bidRows.map((row) => ({ id: `bid-${row.price}`, row }));

  return (
    <div className="overflow-hidden rounded-xl border border-border bg-background">
      <div className="flex items-center justify-between border-border border-b px-3 py-2.5">
        <h3 className="font-semibold text-foreground text-sm">Order book</h3>
        <span className="font-mono text-muted-fg text-xs">
          {base.symbol}/{quote.symbol}
        </span>
      </div>

      <div className="grid grid-cols-3 px-3 py-1 text-[10px] text-muted-fg uppercase tracking-wide">
        <span>Price</span>
        <span className="text-right">Size ({base.symbol})</span>
        <span className="text-right">Total ({base.symbol})</span>
      </div>

      {askSlots.map(({ id, row }) => (
        <LevelRow
          key={id}
          row={row}
          side="ask"
          barPct={row ? barPct(row.total) : 0}
          fractionDigits={fractionDigits}
          decimals={base.decimals}
        />
      ))}

      <div className="flex items-center justify-center gap-3 border-border/60 border-y py-1 font-mono text-[10px] text-muted-fg tabular-nums">
        {spread !== null ? (
          <>
            <span>spread {formatPrice(spread, fractionDigits)}</span>
            {spreadPct !== null && <span>{spreadPct.toFixed(3)}%</span>}
          </>
        ) : (
          <span>—</span>
        )}
      </div>

      {bidSlots.map(({ id, row }) => (
        <LevelRow
          key={id}
          row={row}
          side="bid"
          barPct={row ? barPct(row.total) : 0}
          fractionDigits={fractionDigits}
          decimals={base.decimals}
        />
      ))}
    </div>
  );
}

// Live order-book panel for the current pair. Reads the pair from the swap
// store, polls the book from chain via the SDK, and renders nothing until a
// market actually exists for the pair — so it only appears when there is a
// live market to show (and stays out of the layout otherwise).
export function OrderBookPanel({ className }: { className?: string }) {
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const sameToken = fromStablecoin === toStablecoin;

  const { status, view, base, quote } = useOrderBook(
    fromStablecoin,
    toStablecoin,
    !sameToken,
  );

  if (status !== "ready" || !view || !base || !quote) return null;

  return (
    <div className={className}>
      <OrderBookView view={view} base={base} quote={quote} />
    </div>
  );
}
