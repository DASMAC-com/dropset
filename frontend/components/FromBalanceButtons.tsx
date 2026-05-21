"use client";

import * as Popover from "@radix-ui/react-popover";
import { useWalletConnection } from "@solana/react-hooks";
import { useRef, useState } from "react";
import { formatBaseAmount, parseAmountToBase } from "@/lib/balance";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { emit, useAppEvent } from "@/lib/events";
import { useSwapStore } from "@/lib/store";
import { useAllBalances } from "@/lib/useAllBalances";

const PRESET_PERCENTS = [10, 25, 50];

const sanitizePercent = (raw: string): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    v = v.slice(0, firstDot + 1 + 2);
  }
  if (Number.parseFloat(v) > 100) v = "100";
  return v;
};

// Take a percentage (with up to 2 decimal places) of a base-unit balance using
// scaled bigint math so we avoid float precision loss for large balances.
const portionForPercent = (base: bigint, percent: number): bigint => {
  if (percent <= 0) return 0n;
  if (percent >= 100) return base;
  const scaled = BigInt(Math.round(percent * 100));
  return (base * scaled) / 10000n;
};

// bps is 0..10000 (basis points × 100; i.e. percent with 2 decimal places).
// Trims trailing fractional zeros so 25.00% renders as "25%".
const formatPercentFromBps = (bps: bigint): string => {
  const intPart = bps / 100n;
  const fracBps = bps % 100n;
  if (fracBps === 0n) return `${intPart}%`;
  const fracStr = fracBps.toString().padStart(2, "0").replace(/0+$/, "");
  return fracStr ? `${intPart}.${fracStr}%` : `${intPart}%`;
};

export function FromBalanceButtons() {
  const stablecoin = useSwapStore((s) => s.from.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balanceFor, isReady } = useAllBalances();

  const [open, setOpen] = useState(false);
  const [custom, setCustom] = useState("");
  const customRef = useRef<HTMLInputElement>(null);

  // `balanceFor(mint)` returns null when there's no ATA — treat it as 0 for
  // the purpose of the percent math (the buttons stay disabled either way).
  const base = balanceFor(mint) ?? 0n;
  const disabled = !connected || !isReady || base === 0n;

  useAppEvent("applyMaxBalance", () => {
    if (disabled) return;
    setAmount(formatBaseAmount(base, decimals));
  });
  useAppEvent("openBalancePercent", () => {
    if (disabled) return;
    setOpen(true);
  });

  // Derive the trigger label from amount/balance so picking a preset, typing
  // custom, OR typing directly in the amount input all stay consistent. Uses
  // rounded bigint division to avoid 25% rendering as 24.99% after truncation.
  //
  // `amountBase === base` is the exact-max case and is the only path that
  // renders "100%". Any other value — including amounts that would round up
  // to 10000 bps (e.g., balance dust left from a prior swap) — gets capped at
  // 9999 bps so the label reads "99.99%". Without the cap, a one-atomic
  // shortfall after a from/to flip would silently display "100%" even though
  // the user isn't actually spending their entire balance.
  const amountBase = parseAmountToBase(amount, decimals);
  let percentLabel = "%";
  if (base > 0n && amountBase > 0n && amountBase <= base) {
    if (amountBase === base) {
      percentLabel = "100%";
    } else {
      const raw = (amountBase * 10000n + base / 2n) / base;
      const bps = raw >= 10000n ? 9999n : raw;
      if (bps > 0n) percentLabel = formatPercentFromBps(bps);
    }
  }

  const applyPercent = (percent: number) => {
    if (base === 0n) return;
    setAmount(formatBaseAmount(portionForPercent(base, percent), decimals));
  };

  const selectPreset = (percent: number) => {
    applyPercent(percent);
    setCustom("");
    setOpen(false);
  };

  const onCustomChange = (raw: string) => {
    const cleaned = sanitizePercent(raw);
    setCustom(cleaned);
    const num = Number.parseFloat(cleaned);
    if (Number.isFinite(num) && num > 0) applyPercent(num);
  };

  const sharedButtonClass =
    "rounded border border-border bg-background px-2 py-1 font-medium text-muted-fg text-sm transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-border disabled:hover:text-muted-fg";

  return (
    <div className="ml-auto flex shrink-0 items-center gap-1">
      <button
        type="button"
        disabled={disabled}
        onClick={() => applyPercent(100)}
        title={
          connected
            ? `Use max ${stablecoin} amount`
            : "Connect wallet to use balance"
        }
        className={sharedButtonClass}
      >
        Max
      </button>
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger
          type="button"
          disabled={disabled}
          title={
            connected
              ? `Use a percentage of your ${stablecoin} balance`
              : "Connect wallet to use balance"
          }
          className={`${sharedButtonClass} min-w-[2.25rem] tabular-nums`}
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
            onCloseAutoFocus={(e) => {
              // Don't let Radix restore focus to the % trigger — the keyboard
              // focus ring there is distracting after the popover dismisses.
              // Send focus to the from-amount input instead so the user can
              // keep typing or hit the swap button.
              e.preventDefault();
              emit("focusFromAmount");
            }}
            className="z-50 flex items-center gap-1 rounded-xl border border-border bg-background p-1.5 shadow-lg"
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
                    setOpen(false);
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
