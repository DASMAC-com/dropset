import type { Format } from "@number-flow/react";

// Shared <NumberFlow> Format objects. NumberFlow compares format by identity
// to decide whether to reset the rolling-digit animation, so every consumer
// must use these constants rather than building a fresh object per render
// (which would silently kill the animation).

export const FORMATS = {
  // Standard USD with cent precision (used in token rows, swap result).
  usd: {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  } satisfies Format,

  // USD with extra decimals so sub-$1 stablecoin drift (e.g. $0.9987) stays
  // legible while $1.00 still renders as "$1.00".
  usdPrice: {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 6,
  } satisfies Format,

  // Compact USD ("$1.2M") for volume / mcap / liquidity columns.
  usdCompact: {
    notation: "compact",
    style: "currency",
    currency: "USD",
    maximumFractionDigits: 2,
  } satisfies Format,

  // Compact unit-less count ("1.2K") for holder counts and similar.
  countCompact: {
    notation: "compact",
    maximumFractionDigits: 1,
  } satisfies Format,

  // Percent ("12.34%") for yield/APR readouts. `style: "percent"` scales
  // the value ×100, so callers pass a fraction (0.1234 → "12.34%").
  percent: {
    style: "percent",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  } satisfies Format,

  // Signed percent ("+1.20" / "-1.20" / "0.00") — for gain/loss and
  // slippage readouts. The sign carries the meaning (not just color), so
  // colorblind users still see the direction.
  signedPercent: {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
    signDisplay: "exceptZero",
  } satisfies Format,

  // Signed USD ("+$1,234.00" / "-$1,234.00") for gain/loss headlines where
  // the sign should read explicitly, not only via color.
  signedUsd: {
    style: "currency",
    currency: "USD",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
    signDisplay: "exceptZero",
  } satisfies Format,

  // Signed, ×100-scaled percent ("+14.36%") for a return passed as a fraction
  // — `percent`'s scaling plus an explicit sign.
  signedReturn: {
    style: "percent",
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
    signDisplay: "exceptZero",
  } satisfies Format,

  // Significant-digit rate ("0.011286" or "88.6") — used for the swap-pair
  // exchange rate readout where both small and large values are valid.
  rate: {
    maximumSignificantDigits: 6,
    useGrouping: false,
  } satisfies Format,
};
