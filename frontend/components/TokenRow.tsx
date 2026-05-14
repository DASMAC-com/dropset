"use client";

import { useRef } from "react";
import {
  currencyFlag,
  currencyName,
  stablecoinDecimals,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { type Side, useSwapStore } from "@/lib/store";
import { TokenPicker } from "./TokenPicker";

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

export function TokenRow({ side, label }: { side: Side; label: string }) {
  const activeSide = useSwapStore((s) => s.activeSide);
  const currency = useSwapStore((s) => s[side].currency);
  const stablecoin = useSwapStore((s) => s[side].stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const setActiveSide = useSwapStore((s) => s.setActiveSide);

  const inputRef = useRef<HTMLInputElement>(null);
  useAppEvent("focusFromAmount", () => {
    if (side !== "from") return;
    inputRef.current?.focus();
    inputRef.current?.select();
  });

  const active = activeSide === side;
  const activeBorder = side === "to" ? "border-accent-buy" : "border-accent";
  const decimals = stablecoinDecimals(stablecoin);

  return (
    <div
      className={`flex w-full flex-col gap-2 rounded-lg border bg-muted p-4 text-left transition-colors ${
        active ? activeBorder : "border-border"
      }`}
    >
      <div className="flex min-w-0 items-center gap-2">
        <span className="shrink-0 font-medium text-muted-fg text-sm">
          {label}
        </span>
        <span className="flex min-w-0 items-center gap-2 truncate text-base text-muted-fg">
          <span aria-hidden className="text-xl leading-none">
            {currencyFlag(currency)}
          </span>
          <span className="truncate">
            {currencyName(currency)} ({currency})
          </span>
        </span>
      </div>
      <div className="flex items-center gap-2">
        <TokenPicker side={side} />
        {side === "from" ? (
          <input
            ref={inputRef}
            type="text"
            inputMode="decimal"
            value={amount}
            placeholder="0.0"
            onFocus={() => setActiveSide("from")}
            onChange={(e) => setAmount(sanitizeAmount(e.target.value, decimals))}
            className="min-w-0 flex-1 bg-transparent text-right font-mono text-2xl text-foreground outline-none placeholder:text-muted-fg"
          />
        ) : (
          <span className="min-w-0 flex-1 truncate text-right font-mono text-2xl text-muted-fg">
            0.0
          </span>
        )}
      </div>
    </div>
  );
}
