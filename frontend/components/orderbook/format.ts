// Number formatting shared by the depth ladder and the fills tape, so a price
// or a size is written the same way in both panes.

// FX stablecoin pairs span a wide price range (EUR ≈ 1.1, MXN ≈ 0.05,
// IDR ≈ 0.00006), so pick the fraction digits from the price magnitude and
// apply the same count to every row, keeping the price column aligned.
export function priceFractionDigits(price: number): number {
  if (price >= 1000) return 2;
  if (price >= 1) return 4;
  if (price >= 0.01) return 6;
  return 8;
}

export function formatPrice(price: number, fractionDigits: number): string {
  return price.toLocaleString("en-US", {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  });
}

// An atom count as a decimal token amount. Demo sizes are small, so the Number
// conversion is well inside f64's exact-integer range.
export function amountValue(atoms: bigint, decimals: number): number {
  return Number(atoms) / 10 ** decimals;
}

// How many fraction digits `value` needs to show at least `minSigFigs`
// significant digits — never fewer than 2, so ordinary sizes keep their usual
// look, and never more than 8, so one speck of dust can't blow the column out.
//
// Without this a sub-0.01 fill renders as a flat "0.00": a real trade that
// reads like a broken feed. A sweep that clears one level and takes a sliver of
// the next produces exactly that.
export function amountFractionDigits(value: number, minSigFigs = 2): number {
  if (!Number.isFinite(value) || value === 0) return 2;
  const magnitude = Math.floor(Math.log10(Math.abs(value)));
  return Math.min(8, Math.max(2, minSigFigs - 1 - magnitude));
}

// Compact size, matching the ladder's size/total columns. `fractionDigits` is
// the caller's choice so a whole column can share one count and stay aligned —
// a per-row count would jitter the decimal point down the column.
export function formatAmount(
  atoms: bigint,
  decimals: number,
  fractionDigits = 2,
): string {
  return amountValue(atoms, decimals).toLocaleString("en-US", {
    minimumFractionDigits: fractionDigits,
    maximumFractionDigits: fractionDigits,
  });
}

// `HH:MM:SS` in the viewer's locale conventions but always 24-hour, so the
// column width is stable. Takes unix *seconds* (a transaction's `blockTime`).
export function formatClockTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleTimeString("en-US", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}
