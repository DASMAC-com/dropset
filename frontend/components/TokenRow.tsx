"use client";

import { useLayoutEffect, useRef } from "react";
import {
  currencyFlagUrl,
  currencyName,
  stablecoinDecimals,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { type Side, useSwapStore } from "@/lib/store";
import { useUsdQuote } from "@/lib/useUsdQuote";
import { FromBalanceButtons } from "./FromBalanceButtons";
import { MaxSlippageButton } from "./MaxSlippageButton";
import { TokenPicker } from "./TokenPicker";

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

export function TokenRow({ side, label }: { side: Side; label: string }) {
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

  // Prototype to-side stub: to amount = from / 2. Replace with real
  // aggregator output later.
  const fromNum = Number.parseFloat(amount);
  const safeFrom = Number.isFinite(fromNum) ? fromNum : 0;
  const toNum = safeFrom / 2;
  const toDisplay = formatAmount(toNum.toFixed(decimals));

  const fromUsd = useUsdQuote(
    side === "from" ? stablecoin : "",
    side === "from" ? amount : "",
  );
  let quoteDisplay: string;
  if (side === "from") {
    quoteDisplay = fromUsd.value === null ? "$—" : formatUsd(fromUsd.value);
  } else {
    // To-side $ printout still mirrors the to-amount — placeholder until the
    // real from→to quote is wired up.
    quoteDisplay = formatUsd(toNum);
  }

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
      className={`flex w-full flex-col gap-1.5 rounded-lg border bg-muted px-3 pt-1.5 pb-3 text-left transition-colors ${
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
                toNum > 0 ? "text-foreground" : "text-muted-fg"
              }`}
            >
              {toDisplay}
            </span>
          )}
        </div>
        <div className="text-right font-mono text-muted-fg text-sm tabular-nums">
          {quoteDisplay}
        </div>
      </div>
    </div>
  );
}
