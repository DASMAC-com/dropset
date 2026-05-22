"use client";

import NumberFlow from "@number-flow/react";
import { useState } from "react";
import { FORMATS } from "@/lib/formats";
import { ArrowRightLeft, ChevronDown, ChevronUp } from "./icons";

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
  inAmount,
  outAmount,
  fromSymbol,
  toSymbol,
  fromDecimals,
  toDecimals,
  fresh,
}: {
  // null disables the fee dropdown: the rate header still renders, but no
  // chevron or platform-fee row is shown. Callers should pass null when
  // the swap button isn't actionable (or when no fee is configured).
  bps: number | null;
  inAmount: bigint;
  outAmount: bigint;
  fromSymbol: string;
  toSymbol: string;
  fromDecimals: number;
  toDecimals: number;
  // False during the debounce window after a swap-sides or token-pick,
  // when the cached quote still represents the previous pair. We keep the
  // panel mounted (so the layout doesn't pop) but show "—" instead of a
  // wildly-wrong derived rate.
  fresh: boolean;
}) {
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
        {showFeeDropdown ? (
          <button
            type="button"
            onClick={toggleExpanded}
            aria-expanded={expanded}
            aria-label={expanded ? "Hide fees" : "Show fees"}
            className="shrink-0 rounded p-0.5 text-muted-fg transition-colors hover:text-foreground"
          >
            <Chevron size={14} aria-hidden />
          </button>
        ) : null}
      </div>
      {showFeeDropdown && expanded ? (
        <div className="flex items-center justify-between px-1 pb-1 text-xs">
          <span className="text-muted-fg">Platform fee</span>
          <span className="tabular-nums text-foreground">{bps / 100}%</span>
        </div>
      ) : null}
    </div>
  );
}
