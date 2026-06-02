// Tailwind text-color class for a signed PnL figure: green when up, red when
// down, and a caller-chosen neutral for exactly zero. The vault table uses a
// muted zero, the position dialog a foreground zero — hence the parameter.
export const pnlTone = (n: number, zero = "text-foreground"): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : zero;
