// Display conversion and number formatting shared by the depth ladder and the
// trades tape, so a price or a size is written the same way in both panes.
// Both steps live here: turning a chain-native value (raw `Price` bits, atom
// counts) into a human one, and then rendering that number as text.

import { type PriceBits, quoteForBase } from "@dropset/sdk";

// A raw on-chain `Price` in human quote-per-base units: the quote atoms one
// whole base unit buys, de-scaled by the quote mint's decimals.
//
// The decimals adjustment is not optional. A `Price` is an *atoms* ratio, so
// decoding it alone yields quote-atoms-per-base-atom — which only coincides
// with the displayed rate when both mints share a decimal count. Every 6-vs-6
// pair on the board hides the bug; TGBP (9 decimals) against USDC (6) showed
// it as a clean factor of 1000, the pane reading 0.0014 where the pair trades
// at 1.41.
//
// Built on `quoteForBase` rather than `decodePrice` float math, per that
// function's own guidance: it is the exact integer path the on-chain matcher
// and the TUI's `human_price` both take, so the three agree bit for bit
// instead of drifting at the last decimal.
//
// The ratio is probed at PRICE_PROBE_ATOMS base atoms rather than at exactly
// one whole base unit (`10^baseDecimals`). `quoteForBase` floors, so the
// smaller the probe the coarser the answer, and a *low-decimal* base makes
// that probe tiny: at 2 decimals it asks for the quote value of 100 atoms and
// gets back a single floored integer, leaving the price about two significant
// figures. On IDR-scale pairs that collapses adjacent ladder levels onto the
// same number — the pane showed several distinct asks at 0.000056 — while a
// 6-vs-6 pair hides it, exactly like the decimals bug above. Probing far
// above one unit recovers the encoding's full 8 significant digits before the
// floor bites.
//
// The probe is shared verbatim with the TUI's `human_price`, so it is sized
// to fit the `u64` that fork passes: 1e18 is comfortably inside it, and even
// against the largest representable price stays inside the `u128`
// `quoteForBase` returns.
const PRICE_PROBE_ATOMS = 10n ** 18n;

export function humanPrice(
  bits: PriceBits,
  baseDecimals: number,
  quoteDecimals: number,
): number {
  const perProbe = quoteForBase(bits, PRICE_PROBE_ATOMS);
  return (
    (Number(perProbe) / Number(PRICE_PROBE_ATOMS)) *
    (10 ** baseDecimals / 10 ** quoteDecimals)
  );
}

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
