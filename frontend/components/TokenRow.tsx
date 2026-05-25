"use client";

import NumberFlow, { type Format } from "@number-flow/react";
import { useLayoutEffect, useMemo, useRef } from "react";
import {
  currencyFlagUrl,
  currencyName,
  stablecoinDecimals,
  stablecoinMint,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { FORMATS } from "@/lib/formats";
import { groupThousands, sanitizeAmount } from "@/lib/input";
import { type Side, useSwapStore } from "@/lib/store";
import type { DflowQuote } from "@/lib/useDflowQuote";
import {
  type UsdQuote,
  useLiquidityLookup,
  useUsdQuote,
} from "@/lib/useUsdQuote";
import { FromBalanceButtons } from "./FromBalanceButtons";
import { CircleAlert } from "./icons";
import { MaxSlippageButton } from "./MaxSlippageButton";
import { TokenPicker } from "./TokenPicker";
import { WalletBalance } from "./WalletBalance";

export function TokenRow({
  side,
  label,
  quote,
  fromUsd,
  quoteFresh,
}: {
  side: Side;
  label: string;
  // DFlow quote driving the to-side display. Always passed by SwapPanel
  // (which owns the single hook call); the from-side ignores it.
  quote?: DflowQuote;
  // From-side USD value, passed in on the to-side so we can show the
  // slippage % against the input. Ignored on the from-side.
  fromUsd?: UsdQuote;
  // True iff the quote was fetched for the current store mints. When
  // false, the cached `outAmount` is in the previous pair's units — we
  // suppress the derived to-amount and slippage to avoid flashing 1000×
  // wrong values during the debounce window after a swap-sides or
  // token-pick.
  quoteFresh?: boolean;
}) {
  const activeSide = useSwapStore((s) => s.activeSide);
  const currency = useSwapStore((s) => s[side].currency);
  const stablecoin = useSwapStore((s) => s[side].stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const setActiveSide = useSwapStore((s) => s.setActiveSide);

  // Jupiter-derived liquidity signal for the current stablecoin. "illiquid"
  // means Jupiter returned no usable USD reference price — typically because
  // the token has thin or no on-chain depth. This is independent from DFlow's
  // routable check (handled separately by QuoteError); we surface it here
  // as a per-token preventive warning so users see it before they attempt to
  // swap. "unknown" (prefetch not yet completed) suppresses the icon to avoid
  // flashing a warning that resolves to "liquid" a moment later.
  const liquidity = useLiquidityLookup()(stablecoinMint(stablecoin));
  const lowLiquidity = liquidity === "illiquid";

  const inputRef = useRef<HTMLInputElement>(null);
  const caretRef = useRef<number | null>(null);
  useAppEvent("focusFromAmount", () => {
    if (side !== "from") return;
    inputRef.current?.focus();
    inputRef.current?.select();
  });

  useLayoutEffect(() => {
    if (caretRef.current === null || !inputRef.current) return;
    inputRef.current.setSelectionRange(caretRef.current, caretRef.current);
    caretRef.current = null;
  });

  const active = activeSide === side;
  const activeBorder = side === "to" ? "border-accent-buy" : "border-accent";
  const decimals = stablecoinDecimals(stablecoin);
  const formattedAmount = groupThousands(amount);

  // To-side numeric value for <NumberFlow>. Null when there's no quote
  // (loading first time, error, sameToken, zero input) — in those cases
  // the panel renders a static placeholder string instead of an animated
  // number. Also null when the cached quote is for a stale mint pair,
  // since interpreting old atomic units with new decimals could produce
  // values that are off by 1000× or more. `Number(bigint) / 10**decimals`
  // is lossless within JS's safe integer range, which covers every
  // realistic stablecoin amount.
  const toAmountNumber =
    side === "to" &&
    quoteFresh &&
    quote?.outAmount !== undefined &&
    quote.outAmount !== null
      ? Number(quote.outAmount) / 10 ** decimals
      : null;
  // Maximum precision shown — defer to NumberFlow's grouping/decimal
  // handling rather than our own groupThousands() so the rolling digits
  // animate as a single unit.
  // Memoized so identity is stable across renders — NumberFlow uses
  // identity to detect format changes and would otherwise reset its
  // animation every render.
  const toAmountFormat = useMemo<Format>(
    () => ({ maximumFractionDigits: decimals }),
    [decimals],
  );
  // No value to show on the to-side → render the same em-dash placeholder
  // the error / rateLimited states use. Previously rendered "0" / "0.0",
  // which looked like a real (zero) quote — the dash is unambiguous.
  let toPlaceholder = "—";
  if (side === "to" && quote) {
    if (
      (quote.status === "loading" && quote.outAmount === null) ||
      (!quoteFresh && quote.hasQuote)
    )
      toPlaceholder = "…";
    else if (quote.status === "error" || quote.status === "rateLimited")
      toPlaceholder = "—";
  }
  const toIsLive = side === "to" && quote?.status === "ok" && quoteFresh;

  // For USD on the to-side, route the quote's outAmount through Jupiter's
  // price feed so the dollar readout tracks the real expected output, not
  // the typed input. Falls back to "$—" when there's no quote yet.
  const toAmountDecimal =
    toAmountNumber !== null ? toAmountNumber.toString() : "0";
  const sideAmount = side === "from" ? amount : toAmountDecimal;
  const usd = useUsdQuote(stablecoin, sideAmount);

  // Slippage % between the input USD value and the live to-side output USD
  // value. Negative for the typical case (you give up a little to the spread
  // + fees), positive if the route happens to favor you. Gated on a live
  // quote whose mints still match the store (otherwise the to-USD comes
  // from interpreting old atomic units with new decimals and flashes wildly
  // wrong percents during the post-swap debounce) plus non-zero input USD
  // so we don't divide by zero.
  const slippagePercent =
    side === "to" &&
    quoteFresh &&
    quote?.status === "ok" &&
    fromUsd?.value != null &&
    fromUsd.value > 0 &&
    usd.value !== null
      ? ((usd.value - fromUsd.value) / fromUsd.value) * 100
      : null;

  const onAmountChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const raw = e.target.value;
    const caret = e.target.selectionStart ?? raw.length;
    const digitsBeforeCaret = raw
      .slice(0, caret)
      .replace(/[^0-9.]/g, "").length;
    const next = sanitizeAmount(raw.replace(/,/g, ""), decimals);
    const formatted = groupThousands(next);
    let pos = 0;
    let count = 0;
    while (pos < formatted.length && count < digitsBeforeCaret) {
      const ch = formatted[pos];
      if (ch !== undefined && /[0-9.]/.test(ch)) count++;
      pos++;
    }
    caretRef.current = pos;
    setAmount(next);
  };

  return (
    <div
      onPointerDown={() => setActiveSide(side)}
      className={`flex w-full flex-col gap-1.5 rounded-lg border bg-muted p-3 text-left transition-colors ${
        active ? activeBorder : "border-border"
      }`}
    >
      <div className="flex h-[30px] min-w-0 items-center gap-2">
        <span className="shrink-0 font-medium text-muted-fg text-sm">
          {label}
        </span>
        <span className="flex min-w-0 items-center gap-2 truncate text-base text-muted-fg">
          {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
          <img
            src={currencyFlagUrl(currency)}
            alt=""
            aria-hidden
            width={28}
            height={28}
            className="shrink-0"
          />
          <span className="truncate">
            {currencyName(currency)} ({currency})
          </span>
        </span>
        {lowLiquidity && (
          <span
            className="flex shrink-0 items-center text-amber-400"
            title={`Market data unavailable for ${stablecoin}`}
          >
            <CircleAlert size={14} />
          </span>
        )}
        {side === "from" ? <FromBalanceButtons /> : <MaxSlippageButton />}
      </div>
      <div className="flex flex-col">
        <div className="flex items-center gap-2">
          <TokenPicker side={side} />
          {side === "from" ? (
            <input
              ref={inputRef}
              type="text"
              inputMode="decimal"
              value={formattedAmount}
              placeholder="0.0"
              aria-label="Amount to swap"
              data-shortcut-passthrough="true"
              onFocus={() => setActiveSide("from")}
              onChange={onAmountChange}
              className="min-w-0 flex-1 bg-transparent text-right font-mono text-3xl text-foreground outline-none placeholder:text-muted-fg"
            />
          ) : (
            <output
              aria-live="polite"
              aria-label="You will receive"
              className={`flex min-w-0 flex-1 justify-end truncate text-right font-mono text-3xl ${
                toIsLive ? "text-foreground" : "text-muted-fg"
              }`}
            >
              {toAmountNumber !== null ? (
                <NumberFlow value={toAmountNumber} format={toAmountFormat} />
              ) : (
                toPlaceholder
              )}
            </output>
          )}
        </div>
        <div className="mt-2 flex items-center justify-between gap-2 font-mono text-muted-fg text-sm tabular-nums">
          <WalletBalance stablecoin={stablecoin} />
          <span className="ml-auto flex items-baseline gap-1">
            <span>
              {/*
                On the to-side, drop the USD readout entirely when the
                quote is stale (post-swap debounce). Without this the
                NumberFlow would animate down to $0 — because the cached
                outAmount got zeroed out for staleness — and then back up
                once the new quote lands. Unmounting matches the rate
                display's "go away, come back" behavior.
              */}
              {usd.value !== null &&
              (side === "from" || toAmountNumber !== null) ? (
                <NumberFlow value={usd.value} format={FORMATS.usd} />
              ) : (
                "$—"
              )}
            </span>
            {slippagePercent !== null && (
              <NumberFlow
                value={slippagePercent}
                format={FORMATS.signedPercent}
                prefix="("
                suffix="%)"
              />
            )}
          </span>
        </div>
      </div>
    </div>
  );
}
