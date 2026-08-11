"use client";

import NumberFlow from "@number-flow/react";
import { useState } from "react";
import { ArrowRightLeft, ChevronDown, ChevronUp } from "@/components/icons";
import { stablecoinDecimals } from "@/lib/data/currencies";
import { FORMATS } from "@/lib/format/formats";
import { bpsToPercent } from "@/lib/format/percent";
import { RouteModeToggle } from "./RouteModeToggle";

// Sticky preference: once the user collapses the fee panel, keep it
// collapsed across reloads. Re-expanding is also persisted, so the stored
// value tracks the user's last explicit choice rather than being strictly
// one-way. Default (no entry) is expanded.
const EXPANDED_STORAGE_KEY = "platform-fee-expanded";

function readInitialExpanded(): boolean {
  if (typeof window === "undefined") return true;
  const v = window.localStorage.getItem(EXPANDED_STORAGE_KEY);
  return v === null ? true : v === "1";
}

export function PlatformFee({
  bps,
  showRouteToggle,
  inAmount,
  outAmount,
  fromSymbol,
  toSymbol,
  fresh,
}: {
  // null disables the fee dropdown: the rate header still renders, but no
  // chevron or platform-fee row is shown. Callers should pass null when
  // the swap button isn't actionable (or when no fee is configured).
  bps: number | null;
  // Whether a Dropset market exists for this pair, i.e. whether the route
  // switch will render anything. Passed in rather than re-derived because the
  // chevron's visibility depends on it, and only the caller knows before the
  // switch mounts.
  showRouteToggle: boolean;
  inAmount: bigint;
  outAmount: bigint;
  fromSymbol: string;
  toSymbol: string;
  // False during the debounce window after a swap-sides or token-pick,
  // when the cached quote still represents the previous pair. We keep the
  // panel mounted (so the layout doesn't pop) but show "—" instead of a
  // wildly-wrong derived rate.
  fresh: boolean;
}) {
  // Decimals are a function of the symbol — look them up here rather than
  // making every caller pass redundant fields. stablecoinDecimals throws
  // on an unknown symbol so we don't paper over bad data.
  const fromDecimals = stablecoinDecimals(fromSymbol);
  const toDecimals = stablecoinDecimals(toSymbol);
  const [inverted, setInverted] = useState(false);
  // Cumulative angle (not modulo 360) so every click is a fresh 180° spin
  // in the same direction — otherwise the icon would alternate clockwise
  // and counter-clockwise as the boolean toggled back and forth.
  const [invertRotation, setInvertRotation] = useState(0);
  const [expanded, setExpanded] = useState<boolean>(readInitialExpanded);

  const toggleExpanded = () => {
    setExpanded((v) => {
      const next = !v;
      if (typeof window !== "undefined") {
        window.localStorage.setItem(EXPANDED_STORAGE_KEY, next ? "1" : "0");
      }
      return next;
    });
  };

  const inDecimal = Number(inAmount) / 10 ** fromDecimals;
  const outDecimal = Number(outAmount) / 10 ** toDecimals;
  const { base, quote, rate } = inverted
    ? { base: toSymbol, quote: fromSymbol, rate: inDecimal / outDecimal }
    : { base: fromSymbol, quote: toSymbol, rate: outDecimal / inDecimal };

  const showFeeDropdown = bps !== null;
  // The route switch lives inside the disclosure rather than the always-visible
  // rate row: it's a setting, not a readout, and the collapsed row should stay
  // just the rate. So the chevron has to appear whenever *either* row would —
  // gating it on the fee alone would leave the switch unreachable on a market
  // whose ceiling turns fees off.
  const showDisclosure = showFeeDropdown || showRouteToggle;
  const Chevron = expanded ? ChevronUp : ChevronDown;

  return (
    <div className="mt-2">
      <div className="flex items-center justify-between gap-2 px-1 py-1 text-xs">
        <span className="flex items-center gap-1.5">
          <span className="text-muted-fg">Rate</span>
          <span className="font-semibold tabular-nums text-foreground">
            {fresh && Number.isFinite(rate) && rate > 0 ? (
              <>
                1 {base} ≈ <NumberFlow value={rate} format={FORMATS.rate} />{" "}
                {quote}
              </>
            ) : (
              "—"
            )}
          </span>
          <button
            type="button"
            onClick={() => {
              setInverted((v) => !v);
              setInvertRotation((r) => r + 180);
            }}
            aria-label="Invert rate"
            className="shrink-0 rounded p-0.5 text-muted-fg transition-colors hover:text-foreground"
          >
            <ArrowRightLeft
              size={12}
              aria-hidden
              className="transition-transform duration-300 ease-out"
              style={{ transform: `rotate(${invertRotation}deg)` }}
            />
          </button>
        </span>
        <span className="flex shrink-0 items-center gap-2">
          {showDisclosure ? (
            <button
              type="button"
              onClick={toggleExpanded}
              aria-expanded={expanded}
              aria-label={expanded ? "Hide swap details" : "Show swap details"}
              className="shrink-0 rounded p-0.5 text-muted-fg transition-colors hover:text-foreground"
            >
              <Chevron size={14} aria-hidden />
            </button>
          ) : null}
        </span>
      </div>
      {showRouteToggle && expanded ? (
        <div className="px-1 pb-1 text-xs">
          <RouteModeToggle />
        </div>
      ) : null}
      {showFeeDropdown && expanded ? (
        <div className="flex items-center justify-between px-1 pb-1 text-xs">
          <span className="text-muted-fg">Platform fee</span>
          <span className="tabular-nums text-foreground">
            {bpsToPercent(bps)}%
          </span>
        </div>
      ) : null}
    </div>
  );
}
