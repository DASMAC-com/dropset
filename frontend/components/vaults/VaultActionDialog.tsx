"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { useState } from "react";
import { ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import {
  positionPnl,
  type Vault,
  type VaultMarket,
  type VaultPosition,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";

// Format a token amount for display: trim to 6 decimals, drop trailing zeros.
const fmt = (n: number): string =>
  Number.isFinite(n) ? Number(n.toFixed(6)).toLocaleString("en-US") : "";

// Signed USD ("+$1.20" / "-$0.34") for PnL readouts.
const signedUsd = (n: number): string =>
  `${n >= 0 ? "+" : "-"}$${Math.abs(n).toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;

const pnlTone = (n: number): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : "text-muted-fg";

// Manage a single vault position. The mode is implied by whether the user
// already holds a position: with none you deposit a fresh pro-rata basket;
// with one you can only withdraw the whole thing (no partial withdrawals).
// Deposits/withdrawals mutate client-side mock state via the callbacks — the
// real on-chain flow lands with the program (TODO(program)).
export function VaultActionDialog({
  market,
  vault,
  position,
  onDeposit,
  onWithdraw,
  open,
  onOpenChange,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition | null;
  onDeposit: (basket: VaultPosition) => void;
  onWithdraw: () => void;
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

  const submitDeposit = () => {
    if (!validBasket || depositBlocked) return;
    onDeposit({ base, quote });
    onOpenChange(false);
  };
  const submitWithdraw = () => {
    onWithdraw();
    onOpenChange(false);
  };

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
              {position ? "Withdraw" : "Deposit"} · {market.label}
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

            {position ? (
              // Withdraw view — full position only, no partial withdrawals.
              <>
                <div className="flex flex-col gap-2 rounded-md border border-border bg-muted px-3 py-3 text-sm">
                  <div className="flex items-center justify-between">
                    <span className="text-muted-fg">{market.base}</span>
                    <span className="font-mono text-foreground">
                      {fmt(position.base)}
                    </span>
                  </div>
                  <div className="flex items-center justify-between">
                    <span className="text-muted-fg">{market.quote}</span>
                    <span className="font-mono text-foreground">
                      {fmt(position.quote)}
                    </span>
                  </div>
                </div>
                {(() => {
                  const pnl = positionPnl(market, vault, position);
                  return (
                    <div className="flex flex-col gap-1.5 text-xs">
                      <div className="flex items-center justify-between">
                        <span className="text-muted-fg">PnL (excl. FX)</span>
                        <span
                          className={`font-mono tabular-nums ${pnlTone(pnl.exclFx)}`}
                        >
                          {signedUsd(pnl.exclFx)}
                        </span>
                      </div>
                      <div className="flex items-center justify-between">
                        <span className="text-muted-fg">PnL (incl. FX)</span>
                        <span
                          className={`font-mono tabular-nums ${pnlTone(pnl.inclFx)}`}
                        >
                          {signedUsd(pnl.inclFx)}
                        </span>
                      </div>
                    </div>
                  );
                })()}
                <button
                  type="button"
                  onClick={submitWithdraw}
                  className="h-10 rounded-md bg-accent px-3 font-medium text-background text-sm transition-colors hover:opacity-90"
                >
                  Withdraw all
                </button>
                <p className="text-center text-muted-fg text-xs">
                  Withdrawals redeem your entire basket. Partial withdrawals
                  aren't supported.
                </p>
              </>
            ) : (
              // Deposit view — a fresh pro-rata basket. Only reachable when the
              // user has no existing position.
              <>
                <label className="flex flex-col gap-1.5">
                  <span className="text-muted-fg text-xs">
                    Base · {market.base}
                  </span>
                  <div className="relative">
                    <input
                      type="text"
                      inputMode="decimal"
                      value={baseAmount}
                      onChange={(e) => onBaseChange(e.target.value)}
                      placeholder="0.00"
                      disabled={depositBlocked}
                      className="h-10 w-full rounded-md border border-border bg-muted pr-14 pl-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent disabled:cursor-not-allowed disabled:opacity-50"
                    />
                    <button
                      type="button"
                      onClick={() =>
                        onBaseChange(String(Number(maxBase.toFixed(6))))
                      }
                      disabled={depositBlocked || maxBase <= 0}
                      className="-translate-y-1/2 absolute top-1/2 right-2 rounded border border-border bg-background px-1.5 py-0.5 font-medium text-[10px] text-muted-fg uppercase transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      Max
                    </button>
                  </div>
                </label>
                <label className="flex flex-col gap-1.5">
                  <span className="text-muted-fg text-xs">
                    Quote · {market.quote}
                  </span>
                  <div className="relative">
                    <input
                      type="text"
                      inputMode="decimal"
                      value={quoteAmount}
                      onChange={(e) => onQuoteChange(e.target.value)}
                      placeholder="0.00"
                      disabled={depositBlocked}
                      className="h-10 w-full rounded-md border border-border bg-muted pr-14 pl-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent disabled:cursor-not-allowed disabled:opacity-50"
                    />
                    <button
                      type="button"
                      onClick={() =>
                        onQuoteChange(String(Number(maxQuote.toFixed(6))))
                      }
                      disabled={depositBlocked || maxQuote <= 0}
                      className="-translate-y-1/2 absolute top-1/2 right-2 rounded border border-border bg-background px-1.5 py-0.5 font-medium text-[10px] text-muted-fg uppercase transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      Max
                    </button>
                  </div>
                </label>
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
                  Deposit
                </button>
                {depositBlocked && (
                  <p className="text-center text-muted-fg text-xs">
                    {vault.frozen
                      ? "This vault is frozen, so deposits are closed."
                      : "Outside deposits aren't approved for this vault yet."}
                  </p>
                )}
              </>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
