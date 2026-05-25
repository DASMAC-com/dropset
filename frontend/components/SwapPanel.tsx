"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { useEffect } from "react";
import { parseAmountToBase } from "@/lib/balance";
import {
  ALL_STABLECOIN_MINTS,
  stablecoinDecimals,
  stablecoinMint,
} from "@/lib/currencies";
import { PLATFORM_FEE } from "@/lib/env";
import { emit, useAppEvent } from "@/lib/events";
import { useSameToken, useSwapStore, useSwapStoreApi } from "@/lib/store";
import { useSwapNav } from "@/lib/swapUrl";
import { useAllBalances } from "@/lib/useAllBalances";
import { formatAtomic, useDflowQuote } from "@/lib/useDflowQuote";
import { useDflowSwap } from "@/lib/useDflowSwap";
import {
  prefetchAllTokenInfo,
  REFRESH_INTERVAL_MS,
  useUsdQuote,
} from "@/lib/useUsdQuote";
import { PlatformFee } from "./PlatformFee";
import { QuoteError } from "./QuoteError";
import { RateLimitMessage } from "./RateLimitMessage";
import { SwapArrowButton } from "./SwapArrowButton";
import { SwapResult } from "./SwapResult";
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
  const store = useSwapStoreApi();
  const gotoSwap = useSwapNav();
  const fromMint = stablecoinMint(fromStablecoin);
  const toMint = stablecoinMint(toStablecoin);
  const fromDecimals = stablecoinDecimals(fromStablecoin);
  const toDecimals = stablecoinDecimals(toStablecoin);
  const quote = useDflowQuote(fromMint, toMint, fromDecimals, amount);

  // Toggling direction promotes the current quote's output amount into the
  // new input. The promotion logic lives in the store action — it reads
  // `lastFormattedOutAmount` (which this component keeps in sync via the
  // effect below). With no live quote the existing input is kept; the
  // quote hook refires against the flipped pair either way.
  useAppEvent("swapSides", () => {
    store.getState().swapSides();
    const { from, to } = store.getState();
    gotoSwap(from.stablecoin, to.stablecoin);
  });

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
  const {
    balanceFor,
    isReady: balancesReady,
    error: balancesError,
  } = useAllBalances();
  // Balance fetch failed (e.g. RPC rejected the request). Without a known
  // balance, we can't safely run the insufficient-funds check — block the
  // swap entirely rather than let an under-funded tx fail at simulation.
  const balanceUnknown =
    !sameToken && connected && !isConnecting && balancesError !== null;
  // null (no ATA) is just zero balance for the purposes of the insufficient
  // check — there's nothing to spend.
  const fromBalanceBase = balanceFor(fromMint) ?? 0n;
  const amountBase = parseAmountToBase(amount, fromDecimals);
  const insufficient =
    !sameToken &&
    connected &&
    !isConnecting &&
    hasAmount &&
    balancesReady &&
    amountBase > fromBalanceBase;
  // From-side USD value of *the current quote's input*, not the live input
  // field. Used for the to-side slippage display: pairing this against the
  // to-side USD (which derives from quote.outAmount) keeps both sides of the
  // ratio in sync with the same quote, so adding a digit doesn't briefly
  // flash a huge negative slippage while the new quote is in flight.
  const quoteInDecimal =
    quote.inAmount !== null ? formatAtomic(quote.inAmount, fromDecimals) : "0";
  const quoteFromUsd = useUsdQuote(fromStablecoin, quoteInDecimal);
  const swap = useDflowSwap();
  const swapInFlight =
    swap.status === "preparing" ||
    swap.status === "signing" ||
    swap.status === "confirming";
  const dimmed = needsAmount || insufficient || balanceUnknown;
  const disabled = sameToken || isConnecting || swapInFlight || balanceUnknown;

  let label: string;
  let onClick: () => void;
  if (sameToken) {
    label = "Pick a different token";
    onClick = () => {};
  } else if (!connected) {
    label = isConnecting ? "Connecting…" : "Connect Wallet";
    onClick = () => emit("openWalletModal");
  } else if (balanceUnknown) {
    label = "Balance unavailable";
    onClick = () => {};
  } else if (needsAmount) {
    label = "Enter an amount";
    onClick = () => emit("focusFromAmount");
  } else if (insufficient) {
    label = `Insufficient ${fromStablecoin}`;
    onClick = () => emit("focusFromAmount");
  } else if (swap.status === "preparing") {
    label = "Preparing swap…";
    onClick = () => {};
  } else if (swap.status === "signing") {
    label = "Sign in wallet…";
    onClick = () => {};
  } else if (swap.status === "confirming") {
    label = "Confirming…";
    onClick = () => {};
  } else {
    label = "Swap";
    onClick = () => {
      void swap.execute();
    };
  }

  useAppEvent("executeSwap", () => {
    if (disabled) return;
    onClick();
  });

  // Freshness check — the quote hook holds the previous result during
  // its debounce/refetch window, so right after a swap-sides or token-pick
  // the cached `outAmount` is in the OLD mints' units. Consumers must gate
  // any derivation (rate, slippage, to-side amount) on this to avoid
  // briefly displaying 1000× wrong numbers when decimals differ.
  const quoteFresh =
    quote.inputMint === fromMint && quote.outputMint === toMint;

  // Mirror the live to-side amount (as a decimal string in the to-side's
  // units) into the store so picker/swap actions on other pages can
  // promote it on a direction flip — see store.setToken / store.swapSides.
  // Cleared whenever there's no fresh, positive quote so a stale value
  // can't get promoted after the user wipes the amount.
  useEffect(() => {
    if (quoteFresh && quote.outAmount !== null && quote.outAmount > 0n) {
      store
        .getState()
        .setLastFormattedOutAmount(formatAtomic(quote.outAmount, toDecimals));
    } else {
      store.getState().setLastFormattedOutAmount("");
    }
  }, [quoteFresh, quote.outAmount, toDecimals, store]);

  // Two-stage visibility for the rate/fee panel:
  //   - `routeFound` gates the whole section. No two-sided quote = no
  //     route = nothing to display.
  //   - `canSwap` gates the fee dropdown (chevron + platform-fee row)
  //     within an already-visible section. We only advertise the fee at
  //     the moment the user could actually click Swap — earlier states
  //     (needsAmount, insufficient, in-flight) still
  //     show the rate, just without the fee dropdown.
  // The bigint gates also narrow nullable quote fields for the JSX.
  const routeFound =
    quote.inAmount !== null &&
    quote.inAmount > 0n &&
    quote.outAmount !== null &&
    quote.outAmount > 0n;
  const canSwap = label === "Swap" && !disabled && !dimmed;

  return (
    <>
      <div className="relative rounded-xl border border-border p-3">
        <div className="relative flex flex-col gap-[14px]">
          <TokenRow side="from" label="From" />
          <TokenRow
            side="to"
            label="To"
            quote={quote}
            fromUsd={quoteFromUsd}
            quoteFresh={quoteFresh}
          />
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
        {routeFound && quote.inAmount !== null && quote.outAmount !== null ? (
          <PlatformFee
            bps={canSwap && PLATFORM_FEE ? PLATFORM_FEE.bps : null}
            inAmount={quote.inAmount}
            outAmount={quote.outAmount}
            fromSymbol={fromStablecoin}
            toSymbol={toStablecoin}
            fromDecimals={fromDecimals}
            toDecimals={toDecimals}
            fresh={quoteFresh}
          />
        ) : null}
      </div>
      <RateLimitMessage />
      <QuoteError quote={quote} fromMint={fromMint} toMint={toMint} />
      <SwapResult
        status={swap.status}
        result={swap.result}
        error={swap.error}
        onClose={swap.reset}
      />
    </>
  );
}
