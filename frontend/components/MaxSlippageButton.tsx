"use client";

import * as Popover from "@radix-ui/react-popover";
import { useRef, useState } from "react";
import { useAppEvent } from "@/lib/events";
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
  const customRef = useRef<HTMLInputElement>(null);

  // Open + focus the custom input when the `s` shortcut fires. The
  // popover's `onOpenAutoFocus` handles the focus once it's mounted.
  useAppEvent("openSlippage", () => {
    setOpen(true);
  });

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
        className="ml-auto flex shrink-0 items-center gap-1.5 rounded border border-border bg-background px-2 py-1 font-medium text-muted-fg text-sm transition-colors hover:border-accent-buy hover:text-accent-buy"
      >
        <Settings2 size={14} />
        <span>{summary(slippage)}</span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={6}
          onOpenAutoFocus={(e) => {
            // Send focus into the custom input on open so the user can
            // type a value immediately. Radix would otherwise focus the
            // popover container.
            e.preventDefault();
            customRef.current?.focus();
            customRef.current?.select();
          }}
          className="z-50 flex flex-col gap-1.5 rounded-xl border border-border bg-background p-1.5 shadow-lg"
        >
          <div className="px-1 pt-0.5 font-medium text-foreground text-xs">
            Max slippage
          </div>
          <div className="flex items-center gap-1">
            <button
              type="button"
              onClick={selectAuto}
              className={`flex items-center justify-center gap-1 rounded border px-2 py-1 font-medium text-xs transition-colors ${
                slippage.mode === "auto"
                  ? "border-accent-buy text-accent-buy"
                  : "border-border text-muted-fg hover:border-accent-buy hover:text-accent-buy"
              }`}
            >
              {slippage.mode === "auto" && <Check size={10} />}
              Auto
            </button>
            {PRESETS.map((p) => {
              const active = isPresetActive(slippage, p.percent);
              return (
                <button
                  key={p.label}
                  type="button"
                  onClick={() => selectPreset(p.percent)}
                  className={`rounded border px-2 py-1 font-medium text-xs transition-colors ${
                    active
                      ? "border-accent-buy text-accent-buy"
                      : "border-border text-muted-fg hover:border-accent-buy hover:text-accent-buy"
                  }`}
                >
                  {p.label}
                </button>
              );
            })}
            <label className="flex w-16 items-center gap-1 rounded border border-border px-2 py-1 text-xs focus-within:border-accent-buy">
              <input
                ref={customRef}
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
                onKeyDown={(e) => {
                  // "a" shortcut: jump back to Auto without leaving the
                  // popover via the Auto button. Helpful when the user
                  // started typing a custom value then changed their mind.
                  if (e.key === "a" || e.key === "A") {
                    e.preventDefault();
                    selectAuto();
                    return;
                  }
                  if (e.key === "Enter") {
                    e.preventDefault();
                    setOpen(false);
                  }
                }}
                className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground outline-none placeholder:text-muted-fg"
              />
              <span className="text-muted-fg">%</span>
            </label>
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
