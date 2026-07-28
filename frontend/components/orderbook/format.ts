// Number formatting shared by the depth ladder and the trades tape, so a price
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

// How many fraction digits `value` needs to show at least two significant
// digits — never fewer than 2, so ordinary sizes keep their usual look, and
// never more than 8, so one speck of dust can't blow the column out.
//
// Without this a sub-0.01 fill renders as a flat "0.00": a real trade that
// reads like a broken feed. A sweep that clears one level and takes a sliver of
// the next produces exactly that.
const MIN_SIG_FIGS = 2;

export function amountFractionDigits(value: number): number {
  if (!Number.isFinite(value) || value === 0) return 2;
  const magnitude = Math.floor(Math.log10(Math.abs(value)));
  return Math.min(8, Math.max(2, MIN_SIG_FIGS - 1 - magnitude));
}

// Compact size, matching the ladder's size/total columns. `fractionDigits` is
// the caller's choice because the two panes want different things: the ladder
// shares one count across its rows (the default 2) to hold the decimal point in
// column, while the tape derives it per row — a shared count there would be
// recomputed from whichever fills are in the window and visibly flip the whole
// column as dust scrolls through.
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

// `HH:MM:SS`, in the viewer's local time zone but always `en-US` 24-hour
// formatting, so the column width is stable regardless of the browser locale.
// Takes unix *seconds* (a transaction's `blockTime`).
export function formatClockTime(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleTimeString("en-US", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  });
}
