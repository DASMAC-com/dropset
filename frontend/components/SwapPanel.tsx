"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { useEffect } from "react";
import { ALL_STABLECOIN_MINTS } from "@/lib/currencies";
import { emit } from "@/lib/events";
import { useSameToken } from "@/lib/store";
import { prefetchAllTokenInfo, REFRESH_INTERVAL_MS } from "@/lib/useUsdQuote";
import { SwapArrowButton } from "./SwapArrowButton";
import { TokenRow } from "./TokenRow";

export function SwapPanel() {
  const sameToken = useSameToken();
  const { connected, status } = useWalletConnection();

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
  const disabled = sameToken || isConnecting;

  let label: string;
  let onClick: () => void;
  if (sameToken) {
    label = "Pick a different token";
    onClick = () => {};
  } else if (!connected) {
    label = isConnecting ? "Connecting…" : "Connect Wallet";
    onClick = () => emit("openWalletModal");
  } else {
    label = "Swap";
    onClick = () => {};
  }

  return (
    <div className="relative rounded-xl border border-border p-3">
      <div className="relative flex flex-col gap-[14px]">
        <TokenRow side="from" label="From" />
        <TokenRow side="to" label="To" />
        <div className="absolute inset-x-0 top-1/2 z-10 flex -translate-y-1/2 items-center justify-center">
          <SwapArrowButton />
        </div>
      </div>
      <button
        type="button"
        onClick={onClick}
        disabled={disabled}
        title={sameToken ? "Pick a different token on one side" : undefined}
        className="mt-[14px] w-full rounded-lg bg-accent-buy px-4 py-3.5 font-medium text-background text-lg transition-colors hover:bg-accent-buy-hover disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg disabled:hover:bg-muted"
      >
        {label}
      </button>
    </div>
  );
}
