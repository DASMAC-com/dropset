"use client";

import { currencyFlag, currencyName } from "@/lib/currencies";
import { type Side, useSwapStore } from "@/lib/store";
import { TokenPicker } from "./TokenPicker";

export function TokenRow({ side, label }: { side: Side; label: string }) {
  const activeSide = useSwapStore((s) => s.activeSide);
  const currency = useSwapStore((s) => s[side].currency);

  const active = activeSide === side;

  return (
    <div
      className={`flex w-full flex-col gap-2 rounded-lg border bg-muted p-4 text-left transition-colors ${
        active ? "border-accent" : "border-border"
      }`}
    >
      <div className="flex items-center justify-between gap-2">
        <span className="shrink-0 font-medium text-muted-fg text-sm">
          {label}
        </span>
        <span className="flex min-w-0 items-center gap-2 truncate text-base text-muted-fg">
          <span aria-hidden className="text-xl leading-none">
            {currencyFlag(currency)}
          </span>
          <span className="truncate">{currencyName(currency)}</span>
        </span>
      </div>
      <TokenPicker side={side} />
    </div>
  );
}
