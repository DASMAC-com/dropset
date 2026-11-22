import NumberFlow from "@number-flow/react";
import type { ReactNode } from "react";
import { TokenIcon } from "@/components/ui/TokenIcon";
import { stablecoinDecimals } from "@/lib/data/currencies";
import { FORMATS } from "@/lib/format/formats";
import { BPS_SCALE, cappedPercentLabel } from "@/lib/format/percent";

// The helpers the vault dialog's sections share: balance formatting and the
// atoms-to-units conversion, the label / value row, the token chip, the
// percent label and the amount-over-USD stack.

// A wallet / receivable balance: full token precision as needed — up to the
// token's own decimals, trailing zeros trimmed, but at least two places so it
// reads as a currency balance. Matches the swap picker's wallet readout.
export const fmtBalance = (n: number, symbol: string): string =>
  Number.isFinite(n)
    ? n.toLocaleString("en-US", {
        minimumFractionDigits: 2,
        maximumFractionDigits: stablecoinDecimals(symbol),
      })
    : "—";

export const atomsToUnits = (atoms: bigint, symbol: string): number =>
  Number(atoms) / 10 ** stablecoinDecimals(symbol);

export const detailRow = (label: ReactNode, node: ReactNode) => (
  <div className="flex items-center justify-between">
    <span className="text-muted-fg">{label}</span>
    {node}
  </div>
);

// A token chip: round icon + symbol. The single source of truth for how a
// token is shown in the dialog — the holding/withdraw breakdowns and each
// deposit leg all render through this, so the icon size and symbol weight stay
// consistent across the popup.
export const tokenLabel = (symbol: string): ReactNode => (
  <span className="flex shrink-0 items-center gap-1.5 font-mono font-medium text-foreground text-sm">
    <TokenIcon
      symbol={symbol}
      size={16}
      className="rounded-full"
      title={symbol}
    />
    {symbol}
  </span>
);

// Percent label for a fraction in [0, 1]: blank ("%") at zero, "100%" only on
// an exact full amount, else the live percent capped at 99.99% (cappedPercentLabel).
// Shared by the deposit legs and the withdraw control; the swap From row keeps
// its own bigint path for large balances.
export const fractionPercentLabel = (
  fraction: number,
  isFull: boolean,
): string => {
  if (!(fraction > 0)) return "%";
  if (isFull) return "100%";
  const bps = Math.round(fraction * Number(BPS_SCALE));
  return bps > 0 ? cappedPercentLabel(BigInt(bps), false) : "%";
};

// A token amount stacked over its ≈ USD value — used in the holding and
// withdraw breakdowns so each leg shows what it's worth.
export const amountUsd = (
  amount: number,
  symbol: string,
  usd: number,
): ReactNode => (
  <span className="flex flex-col items-end">
    <span className="font-mono text-foreground tabular-nums">
      {fmtBalance(amount, symbol)}
    </span>
    <span className="font-mono text-[10px] text-muted-fg tabular-nums">
      ≈ <NumberFlow value={usd} format={FORMATS.usd} />
    </span>
  </span>
);
