"use client";

import * as Popover from "@radix-ui/react-popover";
import { useRef, useState } from "react";
import { sanitizePercent } from "@/lib/format/input";
import { Z_POPOVER } from "@/lib/ui/dialog";

// Shared "Max + %" balance control used by the swap From row and the vault
// deposit legs. The % trigger shows a caller-derived label — blank ("%") until
// a value is set, then the live percent — and opens a popover of presets plus a
// custom input, exactly like the swap token row. The numeric domain (bigint
// base units vs float token amounts) stays with the caller: this component only
// renders the control and reports back the chosen percent / max.

const PRESET_PERCENTS = [10, 25, 50];

export function BalancePercentControl({
  percentLabel,
  onApplyPercent,
  onApplyMax,
  disabled = false,
  open,
  onOpenChange,
  maxTitle,
  percentTitle,
  onCloseAutoFocus,
  dense = false,
}: {
  // What the % trigger shows: "%" when nothing's selected, else the live
  // percent ("50%"). Derived by the caller from amount ÷ balance.
  percentLabel: string;
  onApplyPercent: (percent: number) => void;
  onApplyMax: () => void;
  disabled?: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  maxTitle?: string;
  percentTitle?: string;
  onCloseAutoFocus?: (e: Event) => void;
  // Compact sizing for the narrow vault deposit legs.
  dense?: boolean;
}) {
  const [custom, setCustom] = useState("");
  const customRef = useRef<HTMLInputElement>(null);

  const onCustomChange = (raw: string) => {
    const cleaned = sanitizePercent(raw);
    setCustom(cleaned);
    const num = Number.parseFloat(cleaned);
    if (Number.isFinite(num) && num > 0) onApplyPercent(num);
  };

  const selectPreset = (percent: number) => {
    onApplyPercent(percent);
    setCustom("");
    onOpenChange(false);
  };

  const pad = dense ? "px-2 py-0.5 text-[10px]" : "px-2 py-1 text-sm";
  const buttonClass = `rounded border border-border bg-background font-medium text-muted-fg transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-border disabled:hover:text-muted-fg ${pad} ${dense ? "uppercase" : ""}`;

  return (
    <div className="flex shrink-0 items-center gap-1">
      <button
        type="button"
        disabled={disabled}
        onClick={onApplyMax}
        title={maxTitle}
        className={buttonClass}
      >
        Max
      </button>
      <Popover.Root open={open} onOpenChange={onOpenChange}>
        <Popover.Trigger
          type="button"
          disabled={disabled}
          title={percentTitle}
          aria-label={percentTitle}
          className={`${buttonClass} ${dense ? "min-w-[2rem]" : "min-w-[2.25rem]"} tabular-nums`}
        >
          {percentLabel}
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            align="end"
            sideOffset={6}
            onOpenAutoFocus={(e) => {
              e.preventDefault();
              customRef.current?.focus();
              customRef.current?.select();
            }}
            onCloseAutoFocus={onCloseAutoFocus}
            className={`${Z_POPOVER} flex items-center gap-1 rounded-xl border border-border bg-background p-1.5 shadow-lg`}
          >
            {PRESET_PERCENTS.map((p) => (
              <button
                key={p}
                type="button"
                onClick={() => selectPreset(p)}
                className="rounded border border-border px-2 py-1 font-medium text-muted-fg text-xs transition-colors hover:border-accent hover:text-accent"
              >
                {p}%
              </button>
            ))}
            <label className="flex w-16 items-center gap-1 rounded border border-border px-2 py-1 text-xs focus-within:border-accent">
              <input
                ref={customRef}
                type="text"
                inputMode="decimal"
                value={custom}
                placeholder="0"
                onChange={(e) => onCustomChange(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    onOpenChange(false);
                  }
                }}
                className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground outline-none placeholder:text-muted-fg"
              />
              <span className="text-muted-fg">%</span>
            </label>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}
