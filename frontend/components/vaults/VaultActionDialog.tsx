"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useState } from "react";
import { ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import { positionBasket, positionPnl } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  type Vault,
  type VaultMarket,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";

// Trim a token amount to 6 decimals, drop trailing zeros, group thousands.
const fmt = (n: number): string =>
  Number.isFinite(n) ? Number(n.toFixed(6)).toLocaleString("en-US") : "";

const pnlTone = (n: number): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : "text-foreground";

// Read-side position detail: entrance amount, current value, net PnL split
// into the yield (fees) and FX-move legs, and the FX-neutral yield % since
// open. See docs/architecture.md → "Depositor positions and cost basis".
function PositionDetail({
  market,
  vault,
  position,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition;
}) {
  const refNow = vaultReserveRatio(vault) ?? position.entryRefPrice;
  const { baseOut, quoteOut } = positionBasket(position, vault);
  const pnl = positionPnl(position, vault, refNow);
  const row = (label: string, node: ReactNode) => (
    <div className="flex items-center justify-between">
      <span className="text-muted-fg">{label}</span>
      {node}
    </div>
  );
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
      {row(
        "Holding",
        <span className="font-mono text-foreground">
          {fmt(baseOut)} {market.base} / {fmt(quoteOut)} {market.quote}
        </span>,
      )}
      {row(
        "Entrance amount",
        <span className="font-mono text-foreground tabular-nums">
          <NumberFlow value={pnl.entranceAmount} format={FORMATS.usd} />
        </span>,
      )}
      {row(
        "Current value",
        <span className="font-mono text-foreground tabular-nums">
          <NumberFlow value={pnl.currentValue} format={FORMATS.usd} />
        </span>,
      )}
      {row(
        "Net PnL",
        <span className={`font-mono tabular-nums ${pnlTone(pnl.netPnl)}`}>
          <NumberFlow value={pnl.netPnl} format={FORMATS.usd} />
        </span>,
      )}
      <div className="flex flex-col gap-1 border-border border-t pt-1.5">
        {row(
          "Yield (fees)",
          <span className={`font-mono tabular-nums ${pnlTone(pnl.yieldPnl)}`}>
            <NumberFlow value={pnl.yieldPnl} format={FORMATS.usd} />
          </span>,
        )}
        {row(
          "FX move",
          <span className={`font-mono tabular-nums ${pnlTone(pnl.fxPnl)}`}>
            <NumberFlow value={pnl.fxPnl} format={FORMATS.usd} />
          </span>,
        )}
      </div>
      {row(
        "Yield since open",
        <span
          className={`font-mono tabular-nums ${pnlTone(pnl.yieldPctSinceOpen)}`}
        >
          <NumberFlow value={pnl.yieldPctSinceOpen} format={FORMATS.percent} />
        </span>,
      )}
    </div>
  );
}

// Manage a single vault position. With no position the user opens one; with a
// position they see its PnL detail and can top off or withdraw the whole
// basket (no partial withdrawals). Read-side only: the buttons don't send an
// on-chain transaction yet (TODO(program)); deposit amounts link pro-rata to
// the vault's reserve ratio.
export function VaultActionDialog({
  market,
  vault,
  position,
  open,
  onOpenChange,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [baseAmount, setBaseAmount] = useState("");
  const [quoteAmount, setQuoteAmount] = useState("");

  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  const onBaseChange = (value: string) => {
    setBaseAmount(value);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setQuoteAmount(
      Number.isFinite(n) ? String(Number((n * ratio).toFixed(6))) : "",
    );
  };
  const onQuoteChange = (value: string) => {
    setQuoteAmount(value);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setBaseAmount(
      Number.isFinite(n) ? String(Number((n / ratio).toFixed(6))) : "",
    );
  };

  // Mock wallet balance ~2% of the pooled reserves, so "Max" fills a plausible
  // pro-rata basket for the vault. Real balances arrive with the wallet
  // integration.
  const MAX_DEPOSIT_FRACTION = 0.02;
  const maxBase = vault.baseReserve * MAX_DEPOSIT_FRACTION;
  const maxQuote = vault.quoteReserve * MAX_DEPOSIT_FRACTION;

  const base = Number.parseFloat(baseAmount);
  const quote = Number.parseFloat(quoteAmount);
  const validBasket = base > 0 && quote > 0;
  const depositBlocked = vault.frozen || !vault.outsideDepositsApproved;
  const depositLabel = position ? "Top off" : "Open position";

  // No on-chain send yet — both actions just close the dialog.
  const submitDeposit = () => {
    if (!validBasket || depositBlocked) return;
    onOpenChange(false);
  };
  const submitWithdraw = () => onOpenChange(false);

  const amountField = (
    label: string,
    symbol: string,
    value: string,
    onChange: (v: string) => void,
    onMax: () => void,
    maxValue: number,
  ) => (
    <label className="flex flex-col gap-1.5">
      <span className="text-muted-fg text-xs">
        {label} · {symbol}
      </span>
      <div className="relative">
        <input
          type="text"
          inputMode="decimal"
          value={value}
          onChange={(e) => onChange(e.target.value)}
          placeholder="0.00"
          disabled={depositBlocked}
          className="h-10 w-full rounded-md border border-border bg-muted pr-14 pl-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent disabled:cursor-not-allowed disabled:opacity-50"
        />
        <button
          type="button"
          onClick={onMax}
          disabled={depositBlocked || maxValue <= 0}
          className="-translate-y-1/2 absolute top-1/2 right-2 rounded border border-border bg-background px-1.5 py-0.5 font-medium text-[10px] text-muted-fg uppercase transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
        >
          Max
        </button>
      </div>
    </label>
  );

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Content
          aria-describedby={undefined}
          className="-translate-x-1/2 -translate-y-1/2 fixed top-1/2 left-1/2 z-50 w-80 rounded-2xl border border-border bg-background shadow-xl"
        >
          <div className="flex items-center justify-between border-border border-b px-5 py-4">
            <Dialog.Title className="font-semibold text-foreground">
              {position ? "Manage" : "Open position"} · {market.label}
            </Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-muted-fg transition-colors hover:bg-muted hover:text-foreground">
              <X size={14} />
            </Dialog.Close>
          </div>

          <div className="flex flex-col gap-4 p-5">
            <div className="flex items-center gap-2 text-muted-fg text-xs">
              <span>Leader</span>
              <span className="font-mono text-foreground">
                {shortenMint(vault.leader)}
              </span>
              <CopyButton value={vault.leader} label="leader address" />
              <a
                href={explorerAddressUrl(vault.leader)}
                target="_blank"
                rel="noopener noreferrer"
                title="View leader on Solscan"
                className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
              >
                <ExternalLink size={12} />
              </a>
            </div>

            {position && (
              <PositionDetail
                market={market}
                vault={vault}
                position={position}
              />
            )}

            {amountField(
              "Base",
              market.base,
              baseAmount,
              onBaseChange,
              () => onBaseChange(String(Number(maxBase.toFixed(6)))),
              maxBase,
            )}
            {amountField(
              "Quote",
              market.quote,
              quoteAmount,
              onQuoteChange,
              () => onQuoteChange(String(Number(maxQuote.toFixed(6)))),
              maxQuote,
            )}
            <p className="text-muted-fg text-xs">
              {ratio === null
                ? "This vault has no reserves yet, so amounts aren't linked."
                : `Amounts fill pro-rata to the vault's reserves. Set ${market.base} or ${market.quote} and the other follows.`}
            </p>

            <button
              type="button"
              onClick={submitDeposit}
              disabled={!validBasket || depositBlocked}
              className="h-10 rounded-md bg-accent px-3 font-medium text-background text-sm transition-colors hover:opacity-90 disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg"
            >
              {depositLabel}
            </button>
            {depositBlocked && (
              <p className="text-center text-muted-fg text-xs">
                {vault.frozen
                  ? "This vault is frozen, so deposits are closed."
                  : "Outside deposits aren't approved for this vault yet."}
              </p>
            )}

            {position && (
              <>
                <button
                  type="button"
                  onClick={submitWithdraw}
                  className="h-10 rounded-md border border-border bg-background px-3 font-medium text-foreground text-sm transition-colors hover:border-accent hover:text-accent"
                >
                  Withdraw all
                </button>
                <p className="text-center text-muted-fg text-xs">
                  Withdrawals redeem your entire basket. Partial withdrawals
                  aren't supported.
                </p>
              </>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
