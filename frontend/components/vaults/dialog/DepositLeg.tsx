import NumberFlow from "@number-flow/react";
import { useState } from "react";
import { Wallet } from "@/components/icons";
import { BalancePercentControl } from "@/components/ui/BalancePercentControl";
import { InfoTooltip } from "@/components/ui/InfoTooltip";
import { stablecoinDecimals } from "@/lib/data/currencies";
import { FORMATS } from "@/lib/format/formats";
import { sanitizeAmount } from "@/lib/format/input";
import { fmtBalance, fractionPercentLabel, tokenLabel } from "./parts";

// Fill an input with a token amount at its FULL precision — exactly the
// token's decimals, padded with trailing zeros. Used for the derived leg and
// the Max / percent fills so e.g. picking base shows the quote to all its
// decimal places. Empty string for non-finite (clears the field).
const padToken = (n: number, symbol: string): string =>
  Number.isFinite(n) ? n.toFixed(stablecoinDecimals(symbol)) : "";

// The % trigger label for a deposit leg, derived from amount ÷ balance.
const depositPercentLabel = (amount: number, max: number): string =>
  fractionPercentLabel(max > 0 ? amount / max : 0, max > 0 && amount >= max);

// One deposit leg: the token chip beside a large amount input, with the leg's
// ≈ USD value and the shared Max / % control inside the card. The row beneath
// the card carries the wallet balance and, on the derived leg, an "Auto" badge
// — that leg is filled to match the other, and editing it makes it the one the
// user is driving.
export function DepositLeg({
  symbol,
  otherSymbol,
  value,
  onChange,
  autoFilled,
  max,
  usdValue,
  disabled,
}: {
  symbol: string;
  // The other leg's symbol, named in the "Auto" tooltip.
  otherSymbol: string;
  value: string;
  onChange: (v: string) => void;
  autoFilled: boolean;
  max: number;
  usdValue: number;
  disabled: boolean;
}) {
  const [pctOpen, setPctOpen] = useState(false);
  const amount = Number.parseFloat(value);
  const applyPercent = (percent: number) =>
    onChange(padToken((max * percent) / 100, symbol));
  return (
    <div className="flex flex-col gap-1.5">
      {/* A div, not a label: the card holds the interactive Max / % control, so
          we don't want a wrapping label stealing clicks to focus the input. */}
      <div className="flex flex-col gap-2 rounded-xl border border-border bg-muted px-3 py-2.5">
        {/* Token chip beside the large amount input. */}
        <div className="flex items-center justify-between gap-2">
          {tokenLabel(symbol)}
          <input
            type="text"
            inputMode="decimal"
            value={value}
            aria-label={`${symbol} amount`}
            onChange={(e) =>
              onChange(
                sanitizeAmount(e.target.value, stablecoinDecimals(symbol)),
              )
            }
            placeholder="0.00"
            disabled={disabled}
            className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground text-lg outline-none placeholder:text-muted-fg disabled:cursor-not-allowed disabled:opacity-50"
          />
        </div>
        {/* Inside the card: the Max / % control (left) and the leg's ≈ USD
            value (right). */}
        <div className="flex items-center justify-between gap-2 font-mono text-[10px] text-muted-fg tabular-nums">
          <BalancePercentControl
            percentLabel={depositPercentLabel(amount, max)}
            onApplyPercent={applyPercent}
            onApplyMax={() => applyPercent(100)}
            disabled={disabled || max <= 0}
            open={pctOpen}
            onOpenChange={setPctOpen}
            dense
            maxTitle={`Use your full ${symbol} balance`}
            percentTitle={`Use a percentage of your ${symbol} balance`}
          />
          <span>
            ≈ <NumberFlow value={usdValue} format={FORMATS.usd} />
          </span>
        </div>
      </div>
      {/* Below the card: the wallet balance (left) and the "Auto" pill (right,
          where the Max / % control used to sit) on the auto-filled leg. min-h
          reserves the pill's height so the row doesn't grow as it pops between
          legs. */}
      <div className="flex min-h-6 items-center justify-between gap-2 font-mono text-[10px] text-muted-fg tabular-nums">
        <span
          className="flex items-center gap-1"
          title={`Your ${symbol} balance`}
        >
          <Wallet size={12} aria-hidden />
          {fmtBalance(max, symbol)} {symbol}
        </span>
        {autoFilled && (
          <span className="inline-flex items-center gap-1 rounded border border-border bg-muted px-1.5 py-0.5 font-medium text-muted-fg uppercase tracking-wide">
            Auto
            <InfoTooltip
              size={11}
              side="top"
              label={`This amount is set automatically from your ${otherSymbol} deposit, and finalized at transaction time.`}
            />
          </span>
        )}
      </div>
    </div>
  );
}
