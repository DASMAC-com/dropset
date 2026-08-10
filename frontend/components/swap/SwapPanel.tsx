"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { useEffect } from "react";
import { RateLimitMessage } from "@/components/chrome/RateLimitMessage";
import { stablecoinDecimals, stablecoinMint } from "@/lib/data/currencies";
import { findVaultMarket } from "@/lib/data/vaults";
import { useFeeVaultExists } from "@/lib/dflow/feeVault";
import { emit, useAppEvent } from "@/lib/events";
import { parseAmountToBase } from "@/lib/format/balance";
import { useAllBalances } from "@/lib/hooks/useAllBalances";
import { useDflowSwap } from "@/lib/hooks/useDflowSwap";
import { useEclobAvailable } from "@/lib/hooks/useEclobAvailable";
import { useEclobQuote } from "@/lib/hooks/useEclobQuote";
import { useEclobSwap } from "@/lib/hooks/useEclobSwap";
import { useRouterQuote } from "@/lib/hooks/useRouterQuote";
import { useTokenInfoRefresh, useUsdQuote } from "@/lib/hooks/useUsdQuote";
import { formatAtomic } from "@/lib/quote";
import { useSameToken, useSwapStore, useSwapStoreApi } from "@/lib/store";
import { useGoToVaultsForPair, useSwapNav } from "@/lib/ui/swapUrl";
import { PlatformFee } from "./PlatformFee";
import { QuoteError } from "./QuoteError";
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
  const routeMode = useSwapStore((s) => s.routeMode);
  const store = useSwapStoreApi();
  const gotoSwap = useSwapNav();
  const goToVaults = useGoToVaultsForPair();
  // The vault market for this pair, if one exists — independent of swap
  // direction (a vault's base/quote are fixed by the market). Drives the
  // "view vaults" link, which only shows when there's actually a vault.
  const vaultMarket = findVaultMarket(fromStablecoin, toStablecoin);
  const fromMint = stablecoinMint(fromStablecoin);
  const toMint = stablecoinMint(toStablecoin);
  const fromDecimals = stablecoinDecimals(fromStablecoin);
  const toDecimals = stablecoinDecimals(toStablecoin);
  // Route the quote through the selected path: "best" asks the SDK router to
  // price our own book against the DFlow aggregator and take the better fill,
  // "eclob" simulates against our own market alone. Both hooks run (rules of
  // hooks), but only the active one fetches — the other is gated to "skipped"
  // — so the eCLOB-only route never calls DFlow.
  const useBestRoute = routeMode === "best";
  // Resolved once per pair and shared with the route toggle, so a pair we
  // don't have a market for doesn't pay market-discovery reads on every tick.
  const eclobAvailable = useEclobAvailable(fromMint, toMint) === "available";
  const routerQuote = useRouterQuote(
    fromMint,
    toMint,
    fromDecimals,
    amount,
    useBestRoute,
    eclobAvailable,
  );
  const eclobQuote = useEclobQuote(
    fromMint,
    toMint,
    fromDecimals,
    amount,
    !useBestRoute,
  );
  const quote = useBestRoute ? routerQuote : eclobQuote;

  // Freshness check — the quote hook holds the previous result during
  // its debounce/refetch window, so right after a swap-sides or token-pick
  // the cached `outAmount` is in the OLD mints' units. Consumers must gate
  // any derivation (rate, slippage, to-side amount, the routed venue) on this
  // to avoid briefly displaying 1000× wrong numbers when decimals differ.
  const quoteFresh =
    quote.inputMint === fromMint && quote.outputMint === toMint;

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
  useTokenInfoRefresh();

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
  // Pick the swap executor to match the venue the router actually chose — not
  // the route *mode*. Under "Best route" our own book can win, and the swap
  // then has to go through our program rather than the aggregator. Both hooks
  // mount; only the selected one's `execute` is ever called (see the Swap
  // button). Before the first fresh quote lands there's nothing to swap yet,
  // so the mode's default venue stands in.
  const dflowSwap = useDflowSwap();
  const eclobSwap = useEclobSwap();
  const routedVenue =
    (quoteFresh ? quote.venue : null) ?? (useBestRoute ? "dflow" : "dropset");
  const swap = routedVenue === "dropset" ? eclobSwap : dflowSwap;
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

  // Whether the platform-fee vault (the fee wallet's ATA) for the to-mint
  // exists on-chain. This is a *DFlow-route* precondition only: its /order
  // endpoint rejects a request whose `feeAccount` is missing, so on that route
  // the fee can only be charged — and so only reported — once the ATA exists.
  // The eCLOB route has no such precondition; its `swap` instruction creates
  // the fee account itself, so the fee is always charged there.
  //
  // Keyed on the venue the router *chose*, not the route mode: under "Best
  // route" our own book can win, and gating on the mode would then impose
  // DFlow's ATA precondition on a swap going through our own program — hiding
  // a fee we really do charge. It also avoids a pointless getAccountInfo on a
  // route that never consults the answer.
  const aggregatorRoute = routedVenue === "dflow";
  const feeVaultExists = useFeeVaultExists(
    toMint,
    aggregatorRoute && canSwap && routeFound,
  );
  // The platform-fee rate to advertise, or null when none is charged.
  //
  // Taken from the *quote* rather than from `PLATFORM_FEE.bps`, because the
  // two can differ: the eCLOB route clamps the configured rate to the market's
  // on-chain `max_platform_fee`, so a market with a lower — or zero — ceiling
  // is quoted and charged less than the env asks for. Reading the config here
  // would advertise a fee the user isn't paying, and on a fees-off market
  // would invent one outright. `quote.platformFeeBps` is by construction the
  // rate the displayed output was computed with.
  //
  // DFlow keeps its extra precondition on top: its /order rejects a missing
  // fee account, so the fee is only charged once that ATA exists.
  const feeBps =
    canSwap && (aggregatorRoute ? feeVaultExists : true)
      ? quote.platformFeeBps
      : null;

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
            bps={feeBps}
            inAmount={quote.inAmount}
            outAmount={quote.outAmount}
            fromSymbol={fromStablecoin}
            toSymbol={toStablecoin}
            fresh={quoteFresh}
          />
        ) : null}
      </div>
      {/* Jump to the Vaults tab pre-filtered to this pair (in the market's own
          base/quote order), shown only when a vault lists the pair. A plain
          sibling so the page's gap-3 spaces it evenly above (card) and below
          (globe). Hidden below `sm` — real mobile devices redirect /vaults
          back to /swap (see MobileSwapRedirect), so the link would be a
          no-op at phone widths. */}
      {vaultMarket && (
        <div className="hidden justify-center sm:flex">
          <button
            type="button"
            onClick={() => goToVaults(vaultMarket.base, vaultMarket.quote)}
            className="text-muted-fg text-xs transition-colors hover:text-foreground"
          >
            View {vaultMarket.base} / {vaultMarket.quote} vaults
          </button>
        </div>
      )}
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
