// Percent math used by balance-percent presets and slippage UI. All
// computations go through scaled BigInt arithmetic so large balances don't
// lose precision to Number.

// Take a percentage (up to 2 decimals) of a base-unit balance. Negative or
// zero percent → 0n. ≥100% → the full base (no overflow).
export const portionForPercent = (base: bigint, percent: number): bigint => {
  if (percent <= 0) return 0n;
  if (percent >= 100) return base;
  // 100x scale lets the caller use 2 decimal places (e.g. 12.34%) and stay
  // exact under bigint multiply.
  const scaled = BigInt(Math.round(percent * BASIS_POINTS_PER_PERCENT));
  return (base * scaled) / BASIS_POINTS_DIVISOR;
};

// bps is 0..10000 (basis points × 100 — percent with 2 decimal places).
// Trims trailing fractional zeros so 25.00% renders as "25%".
export const formatPercentFromBps = (bps: bigint): string => {
  const intPart = bps / BASIS_POINTS_PER_PERCENT_BIG;
  const fracBps = bps % BASIS_POINTS_PER_PERCENT_BIG;
  if (fracBps === 0n) return `${intPart}%`;
  const fracStr = fracBps.toString().padStart(2, "0").replace(/0+$/, "");
  return fracStr ? `${intPart}.${fracStr}%` : `${intPart}%`;
};

const BASIS_POINTS_PER_PERCENT = 100;
const BASIS_POINTS_PER_PERCENT_BIG = 100n;
const BASIS_POINTS_DIVISOR = 10_000n;
// Highest representable two-decimal-place percent below 100% — used when
// the caller wants to display "99.99%" instead of rounding up to 100%.
export const MAX_BELOW_FULL_BPS = 9_999n;

// Number-domain conversions for fee/slippage UI. Kept as plain `number`
// because both inputs are bounded small (bps in [0, 10000], percent in
// [0, 100]) — bigint math here would force every display site to convert
// back to Number for rendering.
export const bpsToPercent = (bps: number): number =>
  bps / BASIS_POINTS_PER_PERCENT;
export const percentToBps = (percent: number): number =>
  Math.round(percent * BASIS_POINTS_PER_PERCENT);

// Scale factor used when projecting an atomic balance onto a 0-10000 bps
// ratio (e.g. "how close to full balance is the typed amount?").
export const BPS_SCALE = BASIS_POINTS_DIVISOR;
