"use client";

import { useSplToken, useWalletConnection } from "@solana/react-hooks";
import { useEffect } from "react";
import { parseAmountToBase } from "@/lib/balance";
import {
  ALL_STABLECOIN_MINTS,
  stablecoinDecimals,
  stablecoinMint,
} from "@/lib/currencies";
import { emit } from "@/lib/events";
import { useSameToken, useSwapStore } from "@/lib/store";
import { useDflowQuote } from "@/lib/useDflowQuote";
import { prefetchAllTokenInfo, REFRESH_INTERVAL_MS } from "@/lib/useUsdQuote";
import { QuoteError } from "./QuoteError";
import { RateLimitMessage } from "./RateLimitMessage";
import { SwapArrowButton } from "./SwapArrowButton";
import { TokenRow } from "./TokenRow";

export function SwapPanel() {
  const sameToken = useSameToken();
  const { connected, status } = useWalletConnection();

  // Pull the from/to selection here (rather than in TokenRow) because the
  // DFlow quote depends on both sides at once. TokenRow gets the resolved
  // quote as a prop so we don't make the hook think it needs two instances.
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);
  const amount = useSwapStore((s) => s.amount);
  const fromMint = stablecoinMint(fromStablecoin);
  const toMint = stablecoinMint(toStablecoin);
  const fromDecimals = stablecoinDecimals(fromStablecoin);
  const quote = useDflowQuote(fromMint, toMint, fromDecimals, amount);

  // One batched Jupiter call on mount warms every stablecoin's USD price so
  // switching tokens doesn't flash "$—" while a per-mint fetch resolves, then
  // a 10 s interval keeps prices fresh while the page is open.
  useEffect(() => {
    prefetchAllTokenInfo(ALL_STABLECOIN_MINTS);
    const id = window.setInterval(() => {
      prefetchAllTokenInfo(ALL_STABLECOIN_MINTS);
    }, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, []);

  const isConnecting = status === "connecting";
  const hasAmount = Number(amount) > 0;
  const needsAmount = !sameToken && connected && !isConnecting && !hasAmount;
  const fromBalance = useSplToken(fromMint);
  const fromBalanceBase = fromBalance.balance?.exists
    ? fromBalance.balance.amount
    : 0n;
  const amountBase = parseAmountToBase(amount, fromDecimals);
  const insufficient =
    !sameToken &&
    connected &&
    !isConnecting &&
    hasAmount &&
    fromBalance.status === "ready" &&
    amountBase > fromBalanceBase;
  const dimmed = needsAmount || insufficient;
  const disabled = sameToken || isConnecting;

  let label: string;
  let onClick: () => void;
  if (sameToken) {
    label = "Pick a different token";
    onClick = () => {};
  } else if (!connected) {
    label = isConnecting ? "Connecting…" : "Connect Wallet";
    onClick = () => emit("openWalletModal");
  } else if (needsAmount) {
    label = "Enter an amount";
    onClick = () => emit("focusFromAmount");
  } else if (insufficient) {
    label = `Insufficient ${fromStablecoin}`;
    onClick = () => emit("focusFromAmount");
  } else {
    label = "Swap";
    onClick = () => {};
  }

  return (
    <>
      <div className="relative rounded-xl border border-border p-3">
        <div className="relative flex flex-col gap-[14px]">
          <TokenRow side="from" label="From" />
          <TokenRow side="to" label="To" quote={quote} />
          <div className="absolute inset-x-0 top-1/2 z-10 flex -translate-y-1/2 items-center justify-center">
            <SwapArrowButton />
          </div>
        </div>
        <button
          type="button"
          onClick={onClick}
          disabled={disabled}
          title={sameToken ? "Pick a different token on one side" : undefined}
          className={`mt-[14px] w-full rounded-lg bg-accent-buy px-4 py-3.5 font-medium text-background text-lg transition-colors hover:bg-accent-buy-hover disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg disabled:hover:bg-muted ${
            dimmed ? "opacity-60 hover:opacity-80" : ""
          }`}
        >
          {label}
        </button>
      </div>
      <RateLimitMessage />
      <QuoteError quote={quote} />
    </>
  );
}
