"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { useState } from "react";
import { BalancePercentControl } from "@/components/ui/BalancePercentControl";
import { stablecoinDecimals, stablecoinMint } from "@/lib/data/currencies";
import { emit, useAppEvent } from "@/lib/events";
import { formatBaseAmount, parseAmountToBase } from "@/lib/format/balance";
import { cappedPercentLabel, portionForPercent } from "@/lib/format/percent";
import { useAllBalances } from "@/lib/hooks/useAllBalances";
import { useSwapStore } from "@/lib/store";

export function FromBalanceButtons() {
  const stablecoin = useSwapStore((s) => s.from.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const setAmount = useSwapStore((s) => s.setAmount);
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balanceFor, isReady } = useAllBalances();

  const [open, setOpen] = useState(false);

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
      if (raw > 0n) percentLabel = cappedPercentLabel(raw, false);
    }
  }

  const applyPercent = (percent: number) => {
    if (base === 0n) return;
    setAmount(formatBaseAmount(portionForPercent(base, percent), decimals));
  };

  return (
    <div className="ml-auto">
      <BalancePercentControl
        percentLabel={percentLabel}
        onApplyPercent={applyPercent}
        onApplyMax={() => applyPercent(100)}
        disabled={disabled}
        open={open}
        onOpenChange={setOpen}
        maxTitle={
          connected
            ? `Use max ${stablecoin} amount`
            : "Connect wallet to use balance"
        }
        percentTitle={
          connected
            ? `Use a percentage of your ${stablecoin} balance`
            : "Connect wallet to use balance"
        }
        onCloseAutoFocus={(e) => {
          // Don't let Radix restore focus to the % trigger — send focus to the
          // from-amount input instead so the user can keep typing.
          e.preventDefault();
          emit("focusFromAmount");
        }}
      />
    </div>
  );
}
