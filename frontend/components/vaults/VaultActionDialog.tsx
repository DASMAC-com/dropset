"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { type ReactNode, useState } from "react";
import { ChevronDown, ExternalLink, Wallet, X } from "@/components/icons";
import { BalancePercentControl } from "@/components/ui/BalancePercentControl";
import { CopyButton } from "@/components/ui/CopyButton";
import { InfoTooltip } from "@/components/ui/InfoTooltip";
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
import { sanitizeAmount } from "@/lib/format/input";
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

// A token chip: round icon + symbol. The single source of truth for how a
// token is shown in the dialog — the holding/withdraw breakdowns and each
// deposit leg all render through this, so the icon size and symbol weight stay
// consistent across the popup.
const tokenLabel = (src: string, symbol: string): ReactNode => (
  <span className="flex shrink-0 items-center gap-1.5 font-mono font-medium text-foreground text-sm">
    <TokenIcon src={src} symbol={symbol} />
    {symbol}
  </span>
);

// The % trigger label for a deposit leg, derived from amount ÷ balance: blank
// ("%") until the leg has a value, "100%" at a full balance, else the live
// percent (capped at 99.99% short of full, sharing the swap row's rule).
const depositPercentLabel = (amount: number, max: number): string => {
  if (!(max > 0) || !(amount > 0)) return "%";
  if (amount >= max) return "100%";
  const bps = Math.round((amount / max) * 10000);
  return bps > 0 ? cappedPercentLabel(BigInt(bps), false) : "%";
};

// A token amount stacked over its ≈ USD value — used in the holding and
// withdraw breakdowns so each leg shows what it's worth.
const amountUsd = (amount: number, symbol: string, usd: number): ReactNode => (
  <span className="flex flex-col items-end">
    <span className="font-mono text-foreground tabular-nums">
      {fmtBalance(amount, symbol)}
    </span>
    <span className="font-mono text-[10px] text-muted-fg tabular-nums">
      ≈ <NumberFlow value={usd} format={FORMATS.usd} />
    </span>
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
          amountUsd(baseOut, market.base, baseOut * market.baseUsd),
        )}
        {detailRow(
          tokenLabel(market.quoteIconUrl, market.quote),
          amountUsd(quoteOut, market.quote, quoteOut * market.quoteUsd),
        )}
      </div>
    </div>
  );
}

// Withdraw a chosen fraction of the position (take-profit). Shares are one
// fungible claim, so a withdrawal is always a pro-rata slice of the whole
// basket — both legs and the realized PnL scale together. 100% redeems
// everything (and, per the protocol, closes the VaultDepositor PDA). Read-side
// only for now: submitting just closes the dialog — this is where the on-chain
// Withdraw instruction will be built and sent when the program is integrated.
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
  // Blank by default — the user picks a percentage (or Max) before anything is
  // withdrawn, mirroring the swap balance control. The label caps at 99.99%
  // short of an exact full redeem (cappedPercentLabel).
  const [percent, setPercent] = useState("");
  const [pctOpen, setPctOpen] = useState(false);
  const parsed = Number.parseFloat(percent);
  const fraction = Number.isFinite(parsed)
    ? Math.min(1, Math.max(0, parsed / 100))
    : 0;
  const isFull = parsed >= 100;
  const percentLabel =
    fraction <= 0
      ? "%"
      : isFull
        ? "100%"
        : cappedPercentLabel(BigInt(Math.round(fraction * 10000)), false);

  const refNow = vaultReserveRatio(vault) ?? position.entryRefPrice;
  const preview = withdrawalPreview(position, vault, refNow, fraction);

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
            tokenLabel(market.baseIconUrl, market.base),
            amountUsd(
              preview.baseOut,
              market.base,
              preview.baseOut * market.baseUsd,
            ),
          )}
          {detailRow(
            tokenLabel(market.quoteIconUrl, market.quote),
            amountUsd(
              preview.quoteOut,
              market.quote,
              preview.quoteOut * market.quoteUsd,
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
        onClick={onSubmit}
        disabled={fraction <= 0}
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

// One deposit leg: the token icon + symbol beside a large amount input, with
// the leg's ≈ USD value below. The wallet balance sits on the row beneath the
// card alongside the shared Max / % control (matching the swap token row). The
// derived leg shows an "Auto" badge — it's filled to match the other leg, and
// editing it just makes it the one the user is driving.
function DepositLeg({
  iconUrl,
  symbol,
  otherSymbol,
  value,
  onChange,
  autoFilled,
  max,
  usdValue,
  disabled,
}: {
  iconUrl: string;
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
          {tokenLabel(iconUrl, symbol)}
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

// Manage a single vault position. With no position the user opens one; with a
// position they see its PnL detail and can top off or withdraw any fraction of
// the basket (take-profit). Read-side only for now: the buttons don't send a
// transaction yet — this is where the on-chain Deposit / Withdraw instructions
// will be built when the program is integrated. Deposit amounts link pro-rata
// to the vault's reserve ratio, and each leg rounds to its own token decimals.
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

  // A held position can deposit (top off) or withdraw; the two forms are
  // mutually exclusive, chosen from a dropdown. A fresh position only deposits.
  const [mode, setMode] = useState<"deposit" | "withdraw">("deposit");

  const base = Number.parseFloat(baseAmount);
  const quote = Number.parseFloat(quoteAmount);
  const validBasket = base > 0 && quote > 0;
  const depositBlocked = vault.frozen || !vault.outsideDepositsApproved;
  const depositLabel = position ? "Top off" : "Open position";

  // Quote-denominated value of the entered basket (the base leg marked at the
  // vault's reserve ratio). Tested against the deposit cap below — the cap is
  // in the same quote/share units, so this stays quote-denominated.
  const depositValue =
    (Number.isFinite(base) ? base : 0) * (ratio ?? 0) +
    (Number.isFinite(quote) ? quote : 0);
  // USD value of the basket, using the mock per-token prices — for display
  // only (the "Total Deposit" headline and per-leg ≈ readouts).
  const baseUsdValue = (Number.isFinite(base) ? base : 0) * market.baseUsd;
  const quoteUsdValue = (Number.isFinite(quote) ? quote : 0) * market.quoteUsd;
  const depositUsd = baseUsdValue + quoteUsdValue;
  // The largest deposit the vault can take before the new shares would dilute
  // the leader below their min_leader_share floor; null when the floor can't
  // bind. A basket past the cap would be rejected on-chain, so we block it and
  // warn rather than let the user submit a doomed deposit.
  const depositCap = maxOutsideDepositValue(vault);
  const floorBreached =
    depositCap !== null && validBasket && depositValue > depositCap;
  const floorPct = Math.round(leaderFloorFraction(vault) * 100);

  // Read-side only: this is where the Deposit transaction will be built and
  // sent once the program is integrated; for now a valid basket just closes.
  const submitDeposit = () => {
    if (!validBasket || depositBlocked || floorBreached) return;
    onOpenChange(false);
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
                <div className="relative">
                  {/* appearance-none + a custom chevron at right-3 so the
                      caret keeps the same margin as the px-3 text on the left
                      (the native caret sat flush against the border). */}
                  <select
                    value={mode}
                    onChange={(e) =>
                      setMode(e.target.value as "deposit" | "withdraw")
                    }
                    className="h-9 w-full cursor-pointer appearance-none rounded-md border border-border bg-muted px-3 pr-9 font-medium text-foreground text-sm outline-none focus:border-accent"
                  >
                    <option value="deposit">Deposit</option>
                    <option value="withdraw">Withdraw</option>
                  </select>
                  <ChevronDown
                    size={16}
                    aria-hidden
                    className="-translate-y-1/2 pointer-events-none absolute top-1/2 right-3 text-muted-fg"
                  />
                </div>
              </label>
            )}

            {mode === "deposit" ? (
              <>
                <div className="flex flex-col gap-2">
                  <DepositLeg
                    iconUrl={market.baseIconUrl}
                    symbol={market.base}
                    otherSymbol={market.quote}
                    value={baseAmount}
                    onChange={onBaseChange}
                    autoFilled={activeLeg === "quote"}
                    max={maxBase}
                    usdValue={baseUsdValue}
                    disabled={depositBlocked}
                  />
                  <DepositLeg
                    iconUrl={market.quoteIconUrl}
                    symbol={market.quote}
                    otherSymbol={market.base}
                    value={quoteAmount}
                    onChange={onQuoteChange}
                    autoFilled={activeLeg === "base"}
                    max={maxQuote}
                    usdValue={quoteUsdValue}
                    disabled={depositBlocked}
                  />
                  {/* Total of both legs, quote-denominated — the headline
                      figure from the attached deposit card. */}
                  <div className="flex items-center justify-between gap-2 border-border border-t pt-2">
                    <span className="font-medium text-foreground text-sm">
                      Total Deposit
                    </span>
                    <span className="font-mono font-semibold text-foreground text-sm tabular-nums">
                      <NumberFlow value={depositUsd} format={FORMATS.usd} />
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
