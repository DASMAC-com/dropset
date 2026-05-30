"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useState } from "react";
import { ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint, stablecoinDecimals } from "@/lib/data/currencies";
import { positionBasket, positionPnl, withdrawalPreview } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  type Vault,
  type VaultMarket,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";

// Format / round a token amount to that token's own decimals, so EURC shows
// its 6 places and a 2-decimal stable shows 2. Grouping on; trailing zeros
// trimmed (maximumFractionDigits doesn't pad).
const fmtToken = (n: number, symbol: string): string =>
  Number.isFinite(n)
    ? n.toLocaleString("en-US", {
        maximumFractionDigits: stablecoinDecimals(symbol),
      })
    : "";
const roundToken = (n: number, symbol: string): number =>
  Number(n.toFixed(stablecoinDecimals(symbol)));

const pnlTone = (n: number): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : "text-foreground";

const detailRow = (label: string, node: ReactNode) => (
  <div className="flex items-center justify-between">
    <span className="text-muted-fg">{label}</span>
    {node}
  </div>
);

// Read-side position detail: entrance amount, current value, net PnL split into
// the yield (spread capture) and FX-move legs, and the FX-neutral yield % since
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
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
      {detailRow(
        "Holding",
        <span className="font-mono text-foreground">
          {fmtToken(baseOut, market.base)} {market.base} /{" "}
          {fmtToken(quoteOut, market.quote)} {market.quote}
        </span>,
      )}
      {detailRow(
        "Entrance amount",
        <span className="font-mono text-foreground tabular-nums">
          <NumberFlow value={pnl.entranceAmount} format={FORMATS.usd} />
        </span>,
      )}
      {detailRow(
        "Current value",
        <span className="font-mono text-foreground tabular-nums">
          <NumberFlow value={pnl.currentValue} format={FORMATS.usd} />
        </span>,
      )}
      {detailRow(
        "Net PnL",
        <span className={`font-mono tabular-nums ${pnlTone(pnl.netPnl)}`}>
          <NumberFlow value={pnl.netPnl} format={FORMATS.usd} />
        </span>,
      )}
      <div className="flex flex-col gap-1 border-border border-t pt-1.5">
        {detailRow(
          "Yield (spread)",
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
      </div>
      {detailRow(
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

// Withdraw a chosen fraction of the position (take-profit). Shares are one
// fungible claim, so a withdrawal is always a pro-rata slice of the whole
// basket — both legs and the realized PnL scale together. 100% redeems
// everything (and, per the protocol, closes the VaultDepositor PDA). Read-side
// only: submit just closes (TODO(program)).
function WithdrawSection({
  market,
  vault,
  position,
  onSubmit,
}: {
  market: VaultMarket;
  vault: Vault;
  position: VaultPosition;
  onSubmit: () => void;
}) {
  const [fraction, setFraction] = useState(1);
  const refNow = vaultReserveRatio(vault) ?? position.entryRefPrice;
  const preview = withdrawalPreview(position, vault, refNow, fraction);
  const PERCENTS = [25, 50, 75, 100];
  return (
    <div className="flex flex-col gap-3 border-border border-t pt-4">
      <span className="text-muted-fg text-xs">Withdraw</span>
      <div className="flex gap-1">
        {PERCENTS.map((p) => {
          const active = Math.abs(fraction - p / 100) < 1e-9;
          return (
            <button
              key={p}
              type="button"
              onClick={() => setFraction(p / 100)}
              className={`flex-1 rounded border px-2 py-1 font-medium text-xs transition-colors ${
                active
                  ? "border-accent text-accent"
                  : "border-border text-muted-fg hover:border-accent hover:text-accent"
              }`}
            >
              {p === 100 ? "Max" : `${p}%`}
            </button>
          );
        })}
      </div>
      <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
        <div className="flex flex-col gap-0.5">
          <span className="text-muted-fg">You'll receive ≈</span>
          <span className="font-mono text-foreground">
            <NumberFlow value={preview.value} format={FORMATS.usd} /> (
            {fmtToken(preview.baseOut, market.base)} {market.base} /{" "}
            {fmtToken(preview.quoteOut, market.quote)} {market.quote})
          </span>
        </div>
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
      <button
        type="button"
        onClick={onSubmit}
        disabled={fraction <= 0}
        className="h-10 rounded-md border border-border bg-background px-3 font-medium text-foreground text-sm transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
      >
        {fraction >= 1 ? "Withdraw all" : "Withdraw"}
      </button>
    </div>
  );
}

// Manage a single vault position. With no position the user opens one; with a
// position they see its PnL detail and can top off or withdraw any fraction of
// the basket (take-profit). Read-side only: the buttons don't send an on-chain
// transaction yet (TODO(program)); deposit amounts link pro-rata to the vault's
// reserve ratio, and each leg rounds to its own token decimals.
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
  // Which leg the user is driving. The other follows pro-rata and locks, so
  // you set one currency and the basket is determined; clearing it frees both.
  const [activeLeg, setActiveLeg] = useState<"base" | "quote" | null>(null);

  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  const onBaseChange = (value: string) => {
    setBaseAmount(value);
    setActiveLeg(value.trim() ? "base" : null);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setQuoteAmount(
      Number.isFinite(n) ? String(roundToken(n * ratio, market.quote)) : "",
    );
  };
  const onQuoteChange = (value: string) => {
    setQuoteAmount(value);
    setActiveLeg(value.trim() ? "quote" : null);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setBaseAmount(
      Number.isFinite(n) ? String(roundToken(n / ratio, market.base)) : "",
    );
  };

  // Mock wallet balance ~2% of the pooled reserves, so the presets fill a
  // plausible pro-rata basket. Real balances arrive with the wallet
  // integration.
  const MAX_DEPOSIT_FRACTION = 0.02;
  const maxBase = vault.baseReserve * MAX_DEPOSIT_FRACTION;
  const maxQuote = vault.quoteReserve * MAX_DEPOSIT_FRACTION;

  // Percent-of-balance presets, per currency (the linked leg follows pro-rata).
  const PRESET_PERCENTS = [10, 25, 50];

  const base = Number.parseFloat(baseAmount);
  const quote = Number.parseFloat(quoteAmount);
  const validBasket = base > 0 && quote > 0;
  const depositBlocked = vault.frozen || !vault.outsideDepositsApproved;
  const depositLabel = position ? "Top off" : "Open position";

  // No on-chain send yet — actions just close the dialog.
  const submitDeposit = () => {
    if (!validBasket || depositBlocked) return;
    onOpenChange(false);
  };

  const amountField = (
    label: string,
    symbol: string,
    value: string,
    onChange: (v: string) => void,
    max: number,
    disabled: boolean,
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
          disabled={disabled}
          className="h-10 w-full rounded-md border border-border bg-muted pr-14 pl-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent disabled:cursor-not-allowed disabled:opacity-50"
        />
        <button
          type="button"
          onClick={() => onChange(String(roundToken(max, symbol)))}
          disabled={disabled || max <= 0}
          className="-translate-y-1/2 absolute top-1/2 right-2 rounded border border-border bg-background px-1.5 py-0.5 font-medium text-[10px] text-muted-fg uppercase transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
        >
          Max
        </button>
      </div>
      <div className="flex gap-1">
        {PRESET_PERCENTS.map((p) => (
          <button
            key={p}
            type="button"
            onClick={() =>
              onChange(String(roundToken((max * p) / 100, symbol)))
            }
            disabled={disabled || max <= 0}
            className="rounded border border-border bg-background px-2 py-0.5 font-medium text-[10px] text-muted-fg transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-border disabled:hover:text-muted-fg"
          >
            {p}%
          </button>
        ))}
      </div>
    </label>
  );

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-background/80 backdrop-blur-2xl" />
        <Dialog.Content
          aria-describedby={undefined}
          className="-translate-x-1/2 -translate-y-1/2 fixed top-1/2 left-1/2 z-50 w-80 rounded-2xl border border-border bg-background shadow-xl"
        >
          <div className="flex items-center justify-between border-border border-b px-5 py-4">
            <Dialog.Title className="flex items-center gap-2 font-semibold text-foreground">
              <span className="flex shrink-0 items-center gap-1">
                {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
                <img
                  src={market.baseFlagUrl}
                  alt=""
                  aria-hidden
                  width={20}
                  height={20}
                />
                {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
                <img
                  src={market.quoteFlagUrl}
                  alt=""
                  aria-hidden
                  width={20}
                  height={20}
                />
              </span>
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
              maxBase,
              depositBlocked || activeLeg === "quote",
            )}
            {amountField(
              "Quote",
              market.quote,
              quoteAmount,
              onQuoteChange,
              maxQuote,
              depositBlocked || activeLeg === "base",
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
              <WithdrawSection
                market={market}
                vault={vault}
                position={position}
                onSubmit={() => onOpenChange(false)}
              />
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
