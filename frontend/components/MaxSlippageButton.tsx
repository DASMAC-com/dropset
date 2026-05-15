"use client";

import * as Popover from "@radix-ui/react-popover";
import { useState } from "react";
import { type Slippage, useSwapStore } from "@/lib/store";
import { Check, Settings2 } from "./icons";

const PRESETS: { label: string; percent: number }[] = [
  { label: "0.3%", percent: 0.3 },
  { label: "0.5%", percent: 0.5 },
];

const sanitizePercent = (raw: string): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    v = v.slice(0, firstDot + 1 + 2); // cap at 2 decimal places
  }
  return v;
};

const summary = (s: Slippage): string =>
  s.mode === "auto" ? "Auto" : `${s.percent}%`;

const isPresetActive = (s: Slippage, p: number): boolean =>
  s.mode === "fixed" && s.percent === p;

const isCustomActive = (s: Slippage): boolean =>
  s.mode === "fixed" && !PRESETS.some((p) => p.percent === s.percent);

export function MaxSlippageButton() {
  const slippage = useSwapStore((s) => s.slippage);
  const setSlippage = useSwapStore((s) => s.setSlippage);
  const [open, setOpen] = useState(false);
  const [custom, setCustom] = useState<string>(() =>
    isCustomActive(slippage) && slippage.mode === "fixed"
      ? String(slippage.percent)
      : "",
  );

  const selectPreset = (percent: number) => {
    setSlippage({ mode: "fixed", percent });
    setCustom("");
    setOpen(false);
  };

  const selectAuto = () => {
    setSlippage({ mode: "auto" });
    setCustom("");
    setOpen(false);
  };

  const applyCustom = (raw: string) => {
    const cleaned = sanitizePercent(raw);
    setCustom(cleaned);
    const num = Number.parseFloat(cleaned);
    if (Number.isFinite(num) && num > 0) {
      setSlippage({ mode: "fixed", percent: num });
    }
  };

  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger
        type="button"
        title={`Max slippage: ${summary(slippage)}`}
        className="mt-1.5 ml-auto flex shrink-0 items-center gap-1.5 rounded border border-border bg-background px-2 py-1 font-medium text-muted-fg text-sm transition-colors hover:border-accent-buy hover:text-accent-buy"
      >
        <Settings2 size={14} />
        <span>{summary(slippage)}</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          className="z-50 flex w-56 flex-col gap-2 rounded-xl border border-border bg-background p-3 shadow-lg"
        >
          <div className="font-medium text-foreground text-xs">
            Max slippage
          </div>
          <div className="flex gap-1">
            {PRESETS.map((p) => {
              const active = isPresetActive(slippage, p.percent);
              return (
                <button
                  key={p.label}
                  type="button"
                  onClick={() => selectPreset(p.percent)}
                  className={`flex-1 rounded border px-2 py-1 font-medium text-xs transition-colors ${
                    active
                      ? "border-accent-buy text-accent-buy"
                      : "border-border text-muted-fg hover:border-accent-buy hover:text-accent-buy"
                  }`}
                >
                  {p.label}
                </button>
              );
            })}
            <button
              type="button"
              onClick={selectAuto}
              className={`flex flex-1 items-center justify-center gap-1 rounded border px-2 py-1 font-medium text-xs transition-colors ${
                slippage.mode === "auto"
                  ? "border-accent-buy text-accent-buy"
                  : "border-border text-muted-fg hover:border-accent-buy hover:text-accent-buy"
              }`}
            >
              {slippage.mode === "auto" && <Check size={10} />}
              Auto
            </button>
          </div>
          <label className="flex items-center gap-2 rounded border border-border px-2 py-1 text-xs focus-within:border-accent-buy">
            <span className="text-muted-fg">Custom</span>
            <input
              type="text"
              inputMode="decimal"
              value={custom}
              placeholder="0.00"
              onFocus={() => {
                if (isCustomActive(slippage) && slippage.mode === "fixed") {
                  setCustom(String(slippage.percent));
                }
              }}
              onChange={(e) => applyCustom(e.target.value)}
              className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground outline-none placeholder:text-muted-fg"
            />
            <span className="text-muted-fg">%</span>
          </label>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
