"use client";

import { useLayoutEffect, useRef } from "react";
import {
  currencyFlagUrl,
  currencyName,
  stablecoinDecimals,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { type Side, useSwapStore } from "@/lib/store";
import { type DflowQuote, formatAtomic } from "@/lib/useDflowQuote";
import { flashBg, useFlashOnChange } from "@/lib/useFlashOnChange";
import { useUsdQuote } from "@/lib/useUsdQuote";
import { FromBalanceButtons } from "./FromBalanceButtons";
import { MaxSlippageButton } from "./MaxSlippageButton";
import { TokenPicker } from "./TokenPicker";
import { WalletBalance } from "./WalletBalance";

const formatUsd = (n: number): string =>
  `$${n.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;

const sanitizeAmount = (raw: string, decimals: number): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    if (decimals === 0) v = v.slice(0, firstDot);
    else v = v.slice(0, firstDot + 1 + decimals);
  }
  return v;
};

const formatAmount = (raw: string): string => {
  if (!raw) return "";
  const dot = raw.indexOf(".");
  const intPart = dot === -1 ? raw : raw.slice(0, dot);
  const rest = dot === -1 ? "" : raw.slice(dot);
  const grouped = intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return grouped + rest;
};

export function TokenRow({
  side,
  label,
  quote,
}: {
  side: Side;
  label: string;
  // DFlow quote driving the to-side display. Always passed by SwapPanel
  // (which owns the single hook call); the from-side ignores it.
  quote?: DflowQuote;
}) {
  const activeSide = useSwapStore((s) => s.activeSide);
  const currency = useSwapStore((s) => s[side].currency);
  const stablecoin = useSwapStore((s) => s[side].stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const setActiveSide = useSwapStore((s) => s.setActiveSide);

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
  const formattedAmount = formatAmount(amount);

  // To-side display reflects DFlow's quote when we have one. While a fetch
  // is in flight after a prior resolve we keep the last good number on
  // screen (dimmed) so the UI doesn't blink. "—" / "0" cover the cases
  // where a quote isn't available or wasn't requested.
  let toDisplay = "0";
  let toIsLive = false;
  if (side === "to" && quote) {
    if (quote.outAmount !== null) {
      toDisplay = formatAmount(formatAtomic(quote.outAmount, decimals));
      toIsLive = quote.status === "ok";
    } else if (quote.status === "loading") {
      toDisplay = "…";
    } else if (quote.status === "error" || quote.status === "rateLimited") {
      toDisplay = "—";
    }
  }

  // Flash the to-side number whenever the quote resolves to a new value
  // (debounced fetch after typing, 2 s refresh, token switch, etc.).
  // Same pattern as the /currencies table. We key on the bigint outAmount
  // directly so initial nulls don't trigger a flash, and an unchanged
  // refresh (price didn't move) stays silent.
  const toAmountFlash = useFlashOnChange(
    side === "to" ? (quote?.outAmount ?? null) : null,
  );

  // For USD on the to-side, route the quote's outAmount through Jupiter's
  // price feed so the dollar readout tracks the real expected output, not
  // the typed input. Falls back to "0" when there's no quote yet.
  const toAmountDecimal =
    side === "to" && quote?.outAmount !== null && quote?.outAmount !== undefined
      ? formatAtomic(quote.outAmount, decimals)
      : "0";
  const sideAmount = side === "from" ? amount : toAmountDecimal;
  const usd = useUsdQuote(stablecoin, sideAmount);
  const quoteDisplay = usd.value === null ? "$—" : formatUsd(usd.value);

  // Flash the to-side USD on updates too, matching the amount flash.
  // From-side USD changes on every keystroke, which would strobe the row
  // while typing — keep the from-side silent and let the input itself
  // be the user's feedback for "yes, that landed".
  const usdFlash = useFlashOnChange(side === "to" ? usd.value : null);

  const onAmountChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const raw = e.target.value;
    const caret = e.target.selectionStart ?? raw.length;
    const digitsBeforeCaret = raw
      .slice(0, caret)
      .replace(/[^0-9.]/g, "").length;
    const next = sanitizeAmount(raw.replace(/,/g, ""), decimals);
    const formatted = formatAmount(next);
    let pos = 0;
    let count = 0;
    while (pos < formatted.length && count < digitsBeforeCaret) {
      if (/[0-9.]/.test(formatted[pos])) count++;
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
              data-shortcut-passthrough="true"
              onFocus={() => setActiveSide("from")}
              onChange={onAmountChange}
              className="min-w-0 flex-1 bg-transparent text-right font-mono text-3xl text-foreground outline-none placeholder:text-muted-fg"
            />
          ) : (
            <span
              className={`min-w-0 flex-1 truncate text-right font-mono text-3xl ${
                toIsLive ? "text-foreground" : "text-muted-fg"
              }`}
            >
              {/* Inner span sizes to the text so the flash background
                  hugs the number rather than spanning the whole flex slot. */}
              <span
                className={`rounded px-1 transition-colors duration-300 ${flashBg(toAmountFlash)}`}
              >
                {toDisplay}
              </span>
            </span>
          )}
        </div>
        <div className="mt-2 flex items-center justify-between gap-2 font-mono text-muted-fg text-sm tabular-nums">
          <WalletBalance stablecoin={stablecoin} />
          <span
            className={`ml-auto rounded px-1 transition-colors duration-300 ${flashBg(usdFlash)}`}
          >
            {quoteDisplay}
          </span>
        </div>
      </div>
    </div>
  );
}
