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

// Compact 2-dp size, matching the ladder's size/total columns. Demo sizes are
// small, so the Number conversion is well inside f64's exact-integer range.
export function formatAmount(atoms: bigint, decimals: number): string {
  const value = Number(atoms) / 10 ** decimals;
  return value.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
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
