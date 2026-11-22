import NumberFlow from "@number-flow/react";
import { InfoTooltip } from "@/components/ui/InfoTooltip";
import { allTimePnl, positionBasket, positionPnl } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import { type Vault, type VaultMarket, vaultRefPrice } from "@/lib/data/vaults";
import { realizedVault } from "@/lib/eclob/vaultTx";
import { FORMATS } from "@/lib/format/formats";
import { pnlTone } from "@/lib/ui/pnlTone";
import { amountUsd, detailRow, tokenLabel } from "./parts";

// Read-side position detail: the all-time PnL headline (with return %), the
// deposited / current-cost-basis figures, current value and unrealized net PnL
// split into the yield (spread capture) and FX-move legs, the FX-neutral yield
// % since open, and the per-leg holding. See docs/architecture.md →
// "Depositor positions and cost basis".
export function PositionDetail({
  market,
  vault,
  position,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition;
}) {
  const refNow = vaultRefPrice(vault) ?? position.entryRefPrice;
  // Quote→USD rate for the ≈ readouts; the base leg is valued at refNow into
  // quote first, so every USD figure here shares one basis as Current value.
  const quoteUsd = vault.quoteUsd;
  // Valued against the fee-realized share count, as the withdraw below pays.
  const realized = realizedVault(vault);
  const { baseOut, quoteOut } = positionBasket(position, realized);
  const pnl = positionPnl(position, realized, refNow);
  const at = allTimePnl(position, realized, refNow);
  // grossDeposited only exceeds netDeposits once there's been a withdrawal —
  // until then the two deposit figures are identical, so collapse them.
  const hasWithdrawn = position.grossDeposited > position.netDeposits;
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
      {/* All-time headline: lifetime PnL and the return on every dollar ever
          deposited (grossDeposited, the stable denominator). Stacked + nowrap
          so the value never breaks mid-figure. */}
      <div className="flex flex-col gap-0.5">
        <span className="text-[10px] text-muted-fg uppercase tracking-wide">
          All-time PnL
        </span>
        <span
          className={`whitespace-nowrap font-mono font-semibold text-base tabular-nums ${pnlTone(at.allTimePnl)}`}
        >
          <NumberFlow value={at.allTimePnl} format={FORMATS.signedUsd} /> (
          <NumberFlow value={at.allTimePct} format={FORMATS.signedReturn} />)
        </span>
      </div>
      {hasWithdrawn ? (
        <>
          {detailRow(
            "Total deposited",
            <span className="font-mono text-foreground tabular-nums">
              <NumberFlow
                value={position.grossDeposited}
                format={FORMATS.usd}
              />
            </span>,
          )}
          {detailRow(
            // net_deposits is the basis of the shares still held, not the
            // lifetime total — call it out as the current cost basis.
            "Current cost basis",
            <span className="font-mono text-foreground tabular-nums">
              <NumberFlow value={position.netDeposits} format={FORMATS.usd} />
            </span>,
          )}
        </>
      ) : (
        detailRow(
          "Deposited",
          <span className="font-mono text-foreground tabular-nums">
            <NumberFlow value={position.netDeposits} format={FORMATS.usd} />
          </span>,
        )
      )}

      <div className="flex flex-col gap-1.5 border-border border-t pt-1.5">
        {detailRow(
          "Current value",
          <span className="font-mono text-foreground tabular-nums">
            <NumberFlow value={pnl.currentValue} format={FORMATS.usd} />
          </span>,
        )}
        {detailRow(
          "Net PnL (current)",
          <span className={`font-mono tabular-nums ${pnlTone(pnl.netPnl)}`}>
            <NumberFlow value={pnl.netPnl} format={FORMATS.usd} />
          </span>,
        )}
      </div>
      <div className="flex flex-col gap-1 border-border border-t pt-1.5">
        {detailRow(
          "Yield",
          <span className={`font-mono tabular-nums ${pnlTone(pnl.yieldPnl)}`}>
            <NumberFlow value={pnl.yieldPnl} format={FORMATS.usd} />
          </span>,
        )}
        {detailRow(
          "FX move",
          <span className={`font-mono tabular-nums ${pnlTone(pnl.fxPnl)}`}>
            <NumberFlow value={pnl.fxPnl} format={FORMATS.usd} />
          </span>,
        )}
        {detailRow(
          "Yield since open",
          pnl.yieldPctSinceOpen === null ? (
            <span
              className="flex items-center gap-1 font-mono text-muted-fg"
              title="Undefined while the vault holds only one token"
            >
              —
              <InfoTooltip
                size={11}
                side="top"
                label="The yield index needs both tokens in the vault, so it reads nothing while one leg is empty."
              />
            </span>
          ) : (
            <span
              className={`font-mono tabular-nums ${pnlTone(pnl.yieldPctSinceOpen)}`}
            >
              <NumberFlow
                value={pnl.yieldPctSinceOpen}
                format={FORMATS.percent}
              />
            </span>
          ),
        )}
      </div>
      {/* Current basket, broken out per leg at the bottom (tidier than one
          inline string). */}
      <div className="flex flex-col gap-1.5 border-border border-t pt-1.5">
        <span className="text-[10px] text-muted-fg uppercase tracking-wide">
          Holding
        </span>
        {detailRow(
          tokenLabel(market.base),
          amountUsd(baseOut, market.base, baseOut * refNow * quoteUsd),
        )}
        {detailRow(
          tokenLabel(market.quote),
          amountUsd(quoteOut, market.quote, quoteOut * quoteUsd),
        )}
      </div>
    </div>
  );
}
