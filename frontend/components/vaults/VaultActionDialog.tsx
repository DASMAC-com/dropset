"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useState } from "react";
import { ExternalLink, Wallet, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint, stablecoinDecimals } from "@/lib/data/currencies";
import {
  allTimePnl,
  positionBasket,
  positionPnl,
  withdrawalPreview,
} from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  leaderFloorFraction,
  maxOutsideDepositValue,
  type Vault,
  type VaultMarket,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";
import { sanitizeAmount, sanitizePercent } from "@/lib/format/input";
import { cappedPercentLabel } from "@/lib/format/percent";
import { DIALOG_CONTENT_POSITION, DIALOG_OVERLAY_CLASS } from "@/lib/ui/dialog";

// Fill an input with a token amount at its FULL precision — exactly the
// token's decimals, padded with trailing zeros. Used for the derived leg and
// the Max / percent fills so e.g. picking base shows the quote to all its
// decimal places. Empty string for non-finite (clears the field).
const padToken = (n: number, symbol: string): string =>
  Number.isFinite(n) ? n.toFixed(stablecoinDecimals(symbol)) : "";
// A wallet / receivable balance: full token precision as needed — up to the
// token's own decimals, trailing zeros trimmed, but at least two places so it
// reads as a currency balance. Matches the swap picker's wallet readout.
const fmtBalance = (n: number, symbol: string): string =>
  Number.isFinite(n)
    ? n.toLocaleString("en-US", {
        minimumFractionDigits: 2,
        maximumFractionDigits: stablecoinDecimals(symbol),
      })
    : "—";

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
  const at = allTimePnl(position, vault, refNow);
  // grossDeposited only exceeds netDeposits once there's been a withdrawal —
  // until then the two deposit figures are identical, so collapse them.
  const hasWithdrawn = position.grossDeposited > position.netDeposits;
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-xs">
      {/* All-time headline: lifetime PnL and the return on every dollar ever
          deposited (grossDeposited, the stable denominator). */}
      <div className="flex items-center justify-between gap-2">
        <span className="text-muted-fg">All-time PnL</span>
        <span
          className={`font-mono font-semibold text-sm tabular-nums ${pnlTone(at.allTimePnl)}`}
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
          <span
            className={`font-mono tabular-nums ${pnlTone(pnl.yieldPctSinceOpen)}`}
          >
            <NumberFlow
              value={pnl.yieldPctSinceOpen}
              format={FORMATS.percent}
            />
          </span>,
        )}
      </div>
      {/* Current basket, broken out per leg at the bottom (tidier than one
          inline string). */}
      <div className="flex flex-col gap-1.5 border-border border-t pt-1.5">
        <span className="text-[10px] text-muted-fg uppercase tracking-wide">
          Holding
        </span>
        {detailRow(
          tokenLabel(market.baseIconUrl, market.base),
          <span className="font-mono text-foreground tabular-nums">
            {fmtBalance(baseOut, market.base)}
          </span>,
        )}
        {detailRow(
          tokenLabel(market.quoteIconUrl, market.quote),
          <span className="font-mono text-foreground tabular-nums">
            {fmtBalance(quoteOut, market.quote)}
          </span>,
        )}
      </div>
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
            {fmtBalance(preview.baseOut, market.base)}
          </span>,
        )}
        {detailRow(
          tokenLabel(market.quoteIconUrl, market.quote),
          <span className="font-mono text-foreground tabular-nums">
            {fmtBalance(preview.quoteOut, market.quote)}
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

  // A held position can deposit (top off) or withdraw; the two forms are
  // mutually exclusive, chosen from a dropdown. A fresh position only deposits.
  const [mode, setMode] = useState<"deposit" | "withdraw">("deposit");

  const base = Number.parseFloat(baseAmount);
  const quote = Number.parseFloat(quoteAmount);
  const validBasket = base > 0 && quote > 0;
  const depositBlocked = vault.frozen || !vault.outsideDepositsApproved;
  const depositLabel = position ? "Top off" : "Open position";

  // Quote-denominated value of the entered basket (the base leg marked at the
  // vault's reserve ratio). Tested against the deposit cap below.
  const depositValue =
    (Number.isFinite(base) ? base : 0) * (ratio ?? 0) +
    (Number.isFinite(quote) ? quote : 0);
  // The largest deposit the vault can take before the new shares would dilute
  // the leader below their min_leader_share floor; null when the floor can't
  // bind. A basket past the cap would be rejected on-chain, so we block it and
  // warn rather than let the user submit a doomed deposit.
  const depositCap = maxOutsideDepositValue(vault);
  const floorBreached =
    depositCap !== null && validBasket && depositValue > depositCap;
  const floorPct = Math.round(leaderFloorFraction(vault) * 100);

  // No on-chain send yet — actions just close the dialog.
  const submitDeposit = () => {
    if (!validBasket || depositBlocked || floorBreached) return;
    onOpenChange(false);
  };

  // One deposit leg: the token icon + symbol beside a large amount input with
  // the wallet balance (left, swap-style) and the leg's ≈ USD value (right) on
  // the line below — all inside the "picker" card. The Max / % controls sit
  // OUTSIDE the card, beneath it, so the card's top-left isn't left empty. The
  // derived leg shows a ≈ before its amount.
  const amountField = (
    iconUrl: string,
    symbol: string,
    value: string,
    onChange: (v: string) => void,
    approx: boolean,
    max: number,
    percent: string,
    setPercent: (p: string) => void,
    usdValue: number,
  ) => {
    const setFromPercent = (raw: string) => {
      const p = sanitizePercent(raw);
      setPercent(p);
      const n = Number.parseFloat(p);
      onChange(Number.isFinite(n) ? padToken((max * n) / 100, symbol) : "");
    };
    return (
      <div className="flex flex-col gap-1.5">
        <label className="flex flex-col gap-2 rounded-xl border border-border bg-muted px-3 py-2.5">
          {/* Token icon + symbol beside the large amount input. */}
          <div className="flex items-center justify-between gap-2">
            <span className="flex shrink-0 items-center gap-2">
              {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
              <img
                src={iconUrl}
                alt=""
                aria-hidden
                width={28}
                height={28}
                className="h-7 w-7 rounded-full"
              />
              <span className="font-mono font-semibold text-foreground text-lg">
                {symbol}
              </span>
            </span>
            <span className="flex min-w-0 flex-1 items-center justify-end gap-1">
              {approx && <span className="text-muted-fg text-lg">≈</span>}
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
                className="min-w-0 flex-1 bg-transparent text-right font-mono text-foreground text-lg outline-none placeholder:text-muted-fg disabled:cursor-not-allowed disabled:opacity-50"
              />
            </span>
          </div>
          {/* Wallet balance (left, full decimals as needed) and the leg's ≈ USD
              value (right), mirroring the swap token row's sub-line. */}
          <div className="flex items-center justify-between gap-2 font-mono text-[11px] text-muted-fg tabular-nums">
            <span
              className="flex items-center gap-1"
              title={`Your ${symbol} balance`}
            >
              <Wallet size={12} aria-hidden />
              {fmtBalance(max, symbol)} {symbol}
            </span>
            <span>
              ≈ <NumberFlow value={usdValue} format={FORMATS.usd} />
            </span>
          </div>
        </label>
        {/* Max / % outside the picker card. */}
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
          <label className="flex w-14 items-center gap-1 rounded border border-border bg-background px-2 py-0.5 text-[10px] focus-within:border-accent">
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
      </div>
    );
  };

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className={DIALOG_OVERLAY_CLASS} />
        <Dialog.Content
          aria-describedby={undefined}
          className={`${DIALOG_CONTENT_POSITION} flex w-80 flex-col overflow-y-auto rounded-2xl border border-border bg-background shadow-xl`}
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

            {/* With a position, deposit and withdraw are mutually exclusive —
                pick one from the dropdown so the form stays uncrowded. */}
            {position && (
              <label className="flex flex-col gap-1.5">
                <span className="text-muted-fg text-xs">Action</span>
                <select
                  value={mode}
                  onChange={(e) =>
                    setMode(e.target.value as "deposit" | "withdraw")
                  }
                  className="h-9 cursor-pointer rounded-md border border-border bg-muted px-3 font-medium text-foreground text-sm outline-none focus:border-accent"
                >
                  <option value="deposit">Deposit</option>
                  <option value="withdraw">Withdraw</option>
                </select>
              </label>
            )}

            {mode === "deposit" ? (
              <>
                <div className="flex flex-col gap-2">
                  {amountField(
                    market.baseIconUrl,
                    market.base,
                    baseAmount,
                    onBaseChange,
                    activeLeg === "quote",
                    maxBase,
                    basePercent,
                    setBasePercent,
                    Number.isFinite(base) ? base * (ratio ?? 0) : 0,
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
                    Number.isFinite(quote) ? quote : 0,
                  )}
                  {/* Total of both legs, quote-denominated — the headline
                      figure from the attached deposit card. */}
                  <div className="flex items-center justify-between gap-2 border-border border-t pt-2">
                    <span className="font-medium text-foreground text-sm">
                      Total Deposit
                    </span>
                    <span className="font-mono font-semibold text-foreground tabular-nums">
                      <NumberFlow value={depositValue} format={FORMATS.usd} />
                    </span>
                  </div>
                </div>

                {floorBreached && !depositBlocked && (
                  <p className="rounded-md border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-amber-300 text-xs">
                    Too large. This would push the leader below their {floorPct}
                    % minimum stake, so the transaction would be rejected. Lower
                    the amount.
                  </p>
                )}
                <button
                  type="button"
                  onClick={submitDeposit}
                  disabled={!validBasket || depositBlocked || floorBreached}
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
              </>
            ) : (
              position && (
                <WithdrawSection
                  market={market}
                  vault={vault}
                  position={position}
                  onSubmit={() => onOpenChange(false)}
                />
              )
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
