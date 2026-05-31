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
import { sanitizeAmount, sanitizePercent } from "@/lib/format/input";
import { cappedPercentLabel } from "@/lib/format/percent";

// Format / round a token amount to that token's own decimals, so EURC shows
// its 6 places and a 2-decimal stable shows 2. Grouping on; trailing zeros
// trimmed (maximumFractionDigits doesn't pad).
const fmtToken = (n: number, symbol: string): string =>
  Number.isFinite(n)
    ? n.toLocaleString("en-US", {
        maximumFractionDigits: stablecoinDecimals(symbol),
      })
    : "";
// Fill an input with a token amount at its FULL precision — exactly the
// token's decimals, padded with trailing zeros. Used for the derived leg and
// the Max / percent fills so e.g. picking base shows the quote to all its
// decimal places. Empty string for non-finite (clears the field).
const padToken = (n: number, symbol: string): string =>
  Number.isFinite(n) ? n.toFixed(stablecoinDecimals(symbol)) : "";

const pnlTone = (n: number): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : "text-foreground";

const detailRow = (label: ReactNode, node: ReactNode) => (
  <div className="flex items-center justify-between">
    <span className="text-muted-fg">{label}</span>
    {node}
  </div>
);

// Small round token logo, used in place of the words "Base"/"Quote".
function TokenIcon({ src, symbol }: { src: string; symbol: string }) {
  return (
    // biome-ignore lint/performance/noImgElement: small static icon, no optimization needed
    <img
      src={src}
      alt=""
      aria-hidden
      width={16}
      height={16}
      className="rounded-full"
      title={symbol}
    />
  );
}

// A token label: icon + symbol.
const tokenLabel = (src: string, symbol: string): ReactNode => (
  <span className="flex items-center gap-1 text-foreground">
    <TokenIcon src={src} symbol={symbol} />
    {symbol}
  </span>
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
  // The percent is the configurable amount — type any value, or hit Max to
  // fill 100%. The label caps at 99.99% unless it's an exact full redeem,
  // sharing the swap row's rule (see cappedPercentLabel).
  const [percent, setPercent] = useState("100");
  const parsed = Number.parseFloat(percent);
  const fraction = Number.isFinite(parsed)
    ? Math.min(1, Math.max(0, parsed / 100))
    : 0;
  const isFull = parsed >= 100;
  const label = cappedPercentLabel(
    BigInt(Math.round(fraction * 10000)),
    isFull,
  );

  const refNow = vaultReserveRatio(vault) ?? position.entryRefPrice;
  const preview = withdrawalPreview(position, vault, refNow, fraction);

  return (
    <div className="flex flex-col gap-3 border-border border-t pt-4">
      <div className="flex items-center justify-between">
        <span className="text-muted-fg text-xs">Withdraw</span>
        <div className="flex items-center gap-1">
          <button
            type="button"
            onClick={() => setPercent("100")}
            className="rounded border border-border bg-background px-2 py-1 font-medium text-muted-fg text-xs transition-colors hover:border-accent hover:text-accent"
          >
            Max
          </button>
          <label className="flex w-16 items-center gap-1 rounded border border-border px-2 py-1 text-xs focus-within:border-accent">
            <input
              type="text"
              inputMode="decimal"
              value={percent}
              onChange={(e) => setPercent(sanitizePercent(e.target.value))}
              className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground outline-none"
            />
            <span className="text-muted-fg">%</span>
          </label>
        </div>
      </div>
      <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
        {detailRow(
          "You'll receive ≈",
          <span className="font-mono text-foreground tabular-nums">
            <NumberFlow value={preview.value} format={FORMATS.usd} />
          </span>,
        )}
        {detailRow(
          tokenLabel(market.baseIconUrl, market.base),
          <span className="font-mono text-foreground tabular-nums">
            {fmtToken(preview.baseOut, market.base)}
          </span>,
        )}
        {detailRow(
          tokenLabel(market.quoteIconUrl, market.quote),
          <span className="font-mono text-foreground tabular-nums">
            {fmtToken(preview.quoteOut, market.quote)}
          </span>,
        )}
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
        onClick={onSubmit}
        disabled={fraction <= 0}
        className="h-10 rounded-md border border-border bg-background px-3 font-medium text-foreground text-sm transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
      >
        {isFull ? "Withdraw all" : `Withdraw ${label}`}
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
  // Which leg the user is driving (the target). The other stays editable but
  // shows a ≈ to flag that it's derived pro-rata; editing it just flips the
  // target. The exact amount pulled is settled on-chain at deposit time.
  const [activeLeg, setActiveLeg] = useState<"base" | "quote" | null>(null);

  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  const onBaseChange = (value: string) => {
    setBaseAmount(value);
    setActiveLeg(value.trim() ? "base" : null);
    if (ratio === null) return;
    setQuoteAmount(padToken(Number.parseFloat(value) * ratio, market.quote));
  };
  const onQuoteChange = (value: string) => {
    setQuoteAmount(value);
    setActiveLeg(value.trim() ? "quote" : null);
    if (ratio === null) return;
    setBaseAmount(padToken(Number.parseFloat(value) / ratio, market.base));
  };

  // Mock wallet balance ~2% of the pooled reserves, so Max / a percent fills a
  // plausible pro-rata basket. Real balances arrive with the wallet
  // integration.
  const MAX_DEPOSIT_FRACTION = 0.02;
  const maxBase = vault.baseReserve * MAX_DEPOSIT_FRACTION;
  const maxQuote = vault.quoteReserve * MAX_DEPOSIT_FRACTION;

  // Each leg has its own Max / percent control (of that leg's balance); setting
  // either fills it and the other follows pro-rata via onChange.
  const [basePercent, setBasePercent] = useState("");
  const [quotePercent, setQuotePercent] = useState("");

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

  // Icon + symbol stands in for the "Base"/"Quote" label. Input is capped to
  // the token's own decimals; the derived leg shows a ≈ prefix. A Max / percent
  // control sits under each leg (of that leg's mock balance).
  const amountField = (
    iconUrl: string,
    symbol: string,
    value: string,
    onChange: (v: string) => void,
    approx: boolean,
    max: number,
    percent: string,
    setPercent: (p: string) => void,
  ) => {
    const setFromPercent = (raw: string) => {
      const p = sanitizePercent(raw);
      setPercent(p);
      const n = Number.parseFloat(p);
      onChange(Number.isFinite(n) ? padToken((max * n) / 100, symbol) : "");
    };
    return (
      <label className="flex flex-col gap-1.5">
        <span className="text-xs">{tokenLabel(iconUrl, symbol)}</span>
        <div className="relative">
          {approx && (
            <span className="-translate-y-1/2 absolute top-1/2 left-3 text-muted-fg text-sm">
              ≈
            </span>
          )}
          <input
            type="text"
            inputMode="decimal"
            value={value}
            onChange={(e) =>
              onChange(
                sanitizeAmount(e.target.value, stablecoinDecimals(symbol)),
              )
            }
            placeholder="0.00"
            disabled={depositBlocked}
            className={`h-10 w-full rounded-md border border-border bg-muted pr-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent disabled:cursor-not-allowed disabled:opacity-50 ${approx ? "pl-8" : "pl-3"}`}
          />
        </div>
        <div className="flex items-center justify-end gap-1">
          <button
            type="button"
            onClick={() => {
              setPercent("100");
              onChange(padToken(max, symbol));
            }}
            disabled={depositBlocked || max <= 0}
            className="rounded border border-border bg-background px-2 py-0.5 font-medium text-[10px] text-muted-fg uppercase transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-50"
          >
            Max
          </button>
          <label className="flex w-14 items-center gap-1 rounded border border-border px-2 py-0.5 text-[10px] focus-within:border-accent">
            <input
              type="text"
              inputMode="decimal"
              value={percent}
              onChange={(e) => setFromPercent(e.target.value)}
              placeholder="0"
              disabled={depositBlocked}
              className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground outline-none disabled:cursor-not-allowed"
            />
            <span className="text-muted-fg">%</span>
          </label>
        </div>
      </label>
    );
  };

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
              market.baseIconUrl,
              market.base,
              baseAmount,
              onBaseChange,
              activeLeg === "quote",
              maxBase,
              basePercent,
              setBasePercent,
            )}
            {amountField(
              market.quoteIconUrl,
              market.quote,
              quoteAmount,
              onQuoteChange,
              activeLeg === "base",
              maxQuote,
              quotePercent,
              setQuotePercent,
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
