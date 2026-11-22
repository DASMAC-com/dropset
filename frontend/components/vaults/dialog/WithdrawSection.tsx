import NumberFlow from "@number-flow/react";
import { useState } from "react";
import { BalancePercentControl } from "@/components/ui/BalancePercentControl";
import { withdrawalPreview } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import { type Vault, type VaultMarket, vaultRefPrice } from "@/lib/data/vaults";
import {
  realizedVault,
  sharesForFraction,
  sizeWithdraw,
  VAULT_SLIPPAGE_BPS,
} from "@/lib/eclob/vaultTx";
import { FORMATS } from "@/lib/format/formats";
import { BPS_SCALE } from "@/lib/format/percent";
import { pnlTone } from "@/lib/ui/pnlTone";
import {
  amountUsd,
  atomsToUnits,
  detailRow,
  fractionPercentLabel,
  tokenLabel,
} from "./parts";

// Withdraw a chosen fraction of the position (take-profit). Shares are one
// fungible claim, so a withdrawal is always a pro-rata slice of the whole
// basket — both legs and the realized PnL scale together. 100% redeems
// everything (and, per the protocol, closes the VaultDepositor PDA). The legs
// are the program's own floored slice; submitting sends the Withdraw
// instruction, re-sized against a fresh read.
export function WithdrawSection({
  market,
  vault,
  position,
  busy,
  onSubmit,
  onEdit,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition;
  // A transaction is in flight; the button holds until it settles.
  busy: boolean;
  // The fraction to redeem, in basis points (10000 = everything).
  onSubmit: (bps: number) => void;
  // The user picked a new fraction.
  onEdit: () => void;
}) {
  // Blank by default — the user picks a percentage (or Max) before anything is
  // withdrawn, mirroring the swap balance control. The label caps at 99.99%
  // short of an exact full redeem (cappedPercentLabel).
  const [percent, setPercentState] = useState("");
  const setPercent = (p: string) => {
    setPercentState(p);
    onEdit();
  };
  const [pctOpen, setPctOpen] = useState(false);
  const parsed = Number.parseFloat(percent);
  const fraction = Number.isFinite(parsed)
    ? Math.min(1, Math.max(0, parsed / 100))
    : 0;
  const isFull = parsed >= 100;
  const percentLabel = fractionPercentLabel(fraction, isFull);

  const refNow = vaultRefPrice(vault) ?? position.entryRefPrice;
  const quoteUsd = vault.quoteUsd;
  const bps = isFull
    ? Number(BPS_SCALE)
    : Math.round(fraction * Number(BPS_SCALE));
  // The float model sees the fee-realized share count, so the value and PnL
  // figures dilute by the leader's fee just as the legs below do.
  const floatPreview = withdrawalPreview(
    position,
    realizedVault(vault),
    refNow,
    fraction,
  );
  // The legs come from the SDK's floored slice over raw counts, so they never
  // promise an atom the program won't pay; the value and PnL figures stay on
  // the float model, which only they need.
  const slice = sizeWithdraw(
    vault.chain,
    sharesForFraction(BigInt(Math.trunc(position.shares)), bps),
    VAULT_SLIPPAGE_BPS,
  );
  const preview = {
    ...floatPreview,
    baseOut: atomsToUnits(slice.baseOut, market.base),
    quoteOut: atomsToUnits(slice.quoteOut, market.quote),
  };

  return (
    <div className="flex flex-col gap-3 border-border border-t pt-4">
      <div className="flex items-center justify-between">
        <span className="text-muted-fg text-xs">Withdraw</span>
        <BalancePercentControl
          percentLabel={percentLabel}
          onApplyPercent={(p) => setPercent(String(p))}
          onApplyMax={() => setPercent("100")}
          open={pctOpen}
          onOpenChange={setPctOpen}
          dense
          maxTitle="Withdraw your full position"
          percentTitle="Withdraw a percentage of your position"
        />
      </div>
      <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
        {detailRow(
          "You'll receive ≈",
          <span className="font-mono text-foreground tabular-nums">
            <NumberFlow value={preview.value} format={FORMATS.usd} />
          </span>,
        )}
        <div className="flex flex-col gap-1.5">
          {detailRow(
            tokenLabel(market.base),
            amountUsd(
              preview.baseOut,
              market.base,
              preview.baseOut * refNow * quoteUsd,
            ),
          )}
          {detailRow(
            tokenLabel(market.quote),
            amountUsd(
              preview.quoteOut,
              market.quote,
              preview.quoteOut * quoteUsd,
            ),
          )}
        </div>
        <div className="flex flex-col gap-1.5 border-border border-t pt-1.5">
          {detailRow(
            "Realized PnL",
            <span
              className={`font-mono tabular-nums ${pnlTone(preview.realizedPnl)}`}
            >
              <NumberFlow value={preview.realizedPnl} format={FORMATS.usd} />
            </span>,
          )}
          {detailRow(
            "Remaining position",
            <span className="font-mono text-foreground tabular-nums">
              <NumberFlow value={preview.remainingValue} format={FORMATS.usd} />
            </span>,
          )}
        </div>
      </div>
      <button
        type="button"
        onClick={() => onSubmit(bps)}
        disabled={fraction <= 0 || busy}
        className="h-10 rounded-md border border-border bg-background px-3 font-medium text-foreground text-sm transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
      >
        {fraction <= 0
          ? "Withdraw"
          : isFull
            ? "Withdraw all"
            : `Withdraw ${percentLabel}`}
      </button>
    </div>
  );
}
