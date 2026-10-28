"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { useWallet } from "@solana/react-hooks";
import { type ReactNode, useMemo, useState } from "react";
import { ChevronDown, ExternalLink, Wallet, X } from "@/components/icons";
import { BalancePercentControl } from "@/components/ui/BalancePercentControl";
import { CopyButton } from "@/components/ui/CopyButton";
import { InfoTooltip } from "@/components/ui/InfoTooltip";
import { TokenIcon } from "@/components/ui/TokenIcon";
import {
  shortenMint,
  stablecoinDecimals,
  stablecoinMint,
} from "@/lib/data/currencies";
import {
  allTimePnl,
  positionBasket,
  positionPnl,
  withdrawalPreview,
} from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  contributeClosedReason,
  leaderFloorFraction,
  maxOutsideDepositValue,
  type Vault,
  type VaultMarket,
  vaultRefPrice,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import {
  type DepositLeg as DepositLegKind,
  type DepositSizing,
  depositSizingError,
  realizedTotalShares,
  sharesForFraction,
  sizeDeposit,
  sizeWithdraw,
  VAULT_SLIPPAGE_BPS,
} from "@/lib/eclob/vaultTx";
import { EXPLORER_NAME, explorerAddressUrl } from "@/lib/explorer";
import {
  formatBaseAmount,
  groupThousands,
  parseAmountToBase,
} from "@/lib/format/balance";
import { FORMATS } from "@/lib/format/formats";
import { sanitizeAmount } from "@/lib/format/input";
import { BPS_SCALE, cappedPercentLabel } from "@/lib/format/percent";
import { useAllBalances } from "@/lib/hooks/useAllBalances";
import { type UseVaultTx, useVaultTx } from "@/lib/hooks/useVaultTx";
import { DIALOG_CONTENT_POSITION, DIALOG_OVERLAY_CLASS } from "@/lib/ui/dialog";
import { pnlTone } from "@/lib/ui/pnlTone";
import { vaultLinkHref } from "@/lib/ui/swapUrl";

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

// Atoms → a whole-token input string at the token's full precision, the same
// shape padToken gives a float.
const atomsToInput = (atoms: bigint, symbol: string): string => {
  const decimals = stablecoinDecimals(symbol);
  return formatBaseAmount(atoms, decimals, decimals);
};
const atomsToUnits = (atoms: bigint, symbol: string): number =>
  Number(atoms) / 10 ** stablecoinDecimals(symbol);

// A live vault transaction's progress, under its form's button.
function TxStatusLine({ tx }: { tx: UseVaultTx }) {
  const text =
    tx.status === "signing"
      ? "Approve the transaction in your wallet…"
      : tx.status === "confirming"
        ? "Confirming on-chain…"
        : tx.status === "success"
          ? tx.sharesAfter === null
            ? "Confirmed."
            : `Confirmed. You now hold ${groupThousands(tx.sharesAfter.toString())} shares.`
          : tx.status === "error"
            ? tx.error
            : null;
  if (!text) return null;
  const tone =
    tx.status === "error"
      ? "text-red-400"
      : tx.status === "success"
        ? "text-emerald-400"
        : "text-muted-fg";
  return (
    <p role="status" className={`text-center text-xs ${tone}`}>
      {text}
    </p>
  );
}

const detailRow = (label: ReactNode, node: ReactNode) => (
  <div className="flex items-center justify-between">
    <span className="text-muted-fg">{label}</span>
    {node}
  </div>
);

// A token chip: round icon + symbol. The single source of truth for how a
// token is shown in the dialog — the holding/withdraw breakdowns and each
// deposit leg all render through this, so the icon size and symbol weight stay
// consistent across the popup.
const tokenLabel = (symbol: string): ReactNode => (
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
const fractionPercentLabel = (fraction: number, isFull: boolean): string => {
  if (!(fraction > 0)) return "%";
  if (isFull) return "100%";
  const bps = Math.round(fraction * Number(BPS_SCALE));
  return bps > 0 ? cappedPercentLabel(BigInt(bps), false) : "%";
};

// The % trigger label for a deposit leg, derived from amount ÷ balance.
const depositPercentLabel = (amount: number, max: number): string =>
  fractionPercentLabel(max > 0 ? amount / max : 0, max > 0 && amount >= max);

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

// Read-side position detail: the all-time PnL headline (with return %), the
// deposited / current-cost-basis figures, current value and unrealized net PnL
// split into the yield (spread capture) and FX-move legs, the FX-neutral yield
// % since open, and the per-leg holding. See docs/architecture.md →
// "Depositor positions and cost basis".
function PositionDetail({
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

// Withdraw a chosen fraction of the position (take-profit). Shares are one
// fungible claim, so a withdrawal is always a pro-rata slice of the whole
// basket — both legs and the realized PnL scale together. 100% redeems
// everything (and, per the protocol, closes the VaultDepositor PDA). The legs
// are the program's own floored slice; submitting sends the Withdraw
// instruction, re-sized against a fresh read.
function WithdrawSection({
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
  const realizedVault = {
    ...vault,
    totalShares: Number(realizedTotalShares(vault.chain)),
  };
  const floatPreview = withdrawalPreview(
    position,
    realizedVault,
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

// One deposit leg: the token chip beside a large amount input, with the leg's
// ≈ USD value and the shared Max / % control inside the card. The row beneath
// the card carries the wallet balance and, on the derived leg, an "Auto" badge
// — that leg is filled to match the other, and editing it makes it the one the
// user is driving.
function DepositLeg({
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

// The vault's performance fee as a percent label, e.g. "10%" for 100,000 ppm.
const perfFeeLabel = (ppm: number): string =>
  `${(ppm / 10_000).toLocaleString("en-US", { maximumFractionDigits: 2 })}%`;

// What a depositor is buying and when it is quoted, in plain words under the
// deposit form: no share math, and the risk line last. The fee line reads the
// vault's own rate.
function ContributeTerms({ vault }: { vault: Vault }) {
  const feePpm = vault.chain.perfFeeRatePpm;
  const terms = [
    "Your deposit is quoted after the leader's next re-center.",
    "Withdraw at any time, paid in both assets.",
    feePpm > 0
      ? `The leader takes a ${perfFeeLabel(feePpm)} performance fee on the vault's gains.`
      : "The leader takes no performance fee.",
  ];
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-muted-fg text-xs">
      <ul className="flex list-disc flex-col gap-1 pl-4">
        {terms.map((t) => (
          <li key={t}>{t}</li>
        ))}
      </ul>
      <p className="border-border border-t pt-1.5 text-foreground">
        The leader controls the ladder and the fee. You can leave at any time.
        You are taking a risk.
      </p>
    </div>
  );
}

// Manage a single vault position. With no position the user opens one; with a
// position they see its PnL detail and can top off or withdraw any fraction of
// the basket (take-profit). The buttons build, sign and send the Deposit /
// Withdraw instruction (lib/eclob/vaultTx.ts) and report the share balance
// read back from chain. Deposit amounts link pro-rata to the vault through
// the program's own single-leg sizing, so the derived leg is exactly what
// will be pulled, and each leg rounds to its own token decimals.
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
  // Which leg the user is driving (the target). The other stays editable but is
  // flagged with an "Auto" badge to show it's derived pro-rata; editing it just
  // flips the target. The exact amount pulled is settled at transaction time.
  const [activeLeg, setActiveLeg] = useState<"base" | "quote" | null>(null);

  const chain = vault.chain;
  const wallet = useWallet();
  const owner =
    wallet.status === "connected"
      ? wallet.session.account.address.toString()
      : null;
  const vaultRef = useMemo(
    () => ({ base: market.base, quote: market.quote, seq: chain.seq }),
    [market.base, market.quote, chain.seq],
  );
  const tx = useVaultTx(vaultRef);
  const busy =
    tx.status === "preparing" ||
    tx.status === "signing" ||
    tx.status === "confirming";

  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  // The deposit sized by the SDK on the leg being driven: the basket the
  // program will pull and the shares it will mint, or why it would reject the
  // amount.
  const sizeLeg = (
    leg: DepositLegKind,
    value: string,
  ): { sizing: DepositSizing } | { error: string } | null => {
    const symbol = leg === "base" ? market.base : market.quote;
    const amountIn = parseAmountToBase(value, stablecoinDecimals(symbol));
    if (amountIn === 0n) return null;
    try {
      return {
        sizing: sizeDeposit(chain, leg, amountIn, VAULT_SLIPPAGE_BPS),
      };
    } catch (e) {
      return { error: depositSizingError(e, leg, market) };
    }
  };

  // The other leg follows the driven one: the exact final the program will
  // pull (blank while it can't be sized).
  const onLegChange = (leg: DepositLegKind, value: string) => {
    const [setOwn, setOther, otherSymbol] =
      leg === "base"
        ? [setBaseAmount, setQuoteAmount, market.quote]
        : [setQuoteAmount, setBaseAmount, market.base];
    setOwn(value);
    setActiveLeg(value.trim() ? leg : null);
    clearTxResult();
    const sized = sizeLeg(leg, value);
    setOther(
      sized && "sizing" in sized
        ? atomsToInput(
            leg === "base" ? sized.sizing.quoteIn : sized.sizing.baseIn,
            otherSymbol,
          )
        : "",
    );
  };
  const onBaseChange = (value: string) => onLegChange("base", value);
  const onQuoteChange = (value: string) => onLegChange("quote", value);

  // Per-leg wallet balance: the connected wallet's token accounts (read as 0
  // until they load or with no wallet; `balancesKnown` below keeps that 0 from
  // reading as a shortfall).
  const baseMint = stablecoinMint(market.base);
  const quoteMint = stablecoinMint(market.quote);
  const balances = useAllBalances([baseMint, quoteMint]);
  const walletAtoms = (mint: string): bigint => balances.balanceFor(mint) ?? 0n;
  const maxBase = atomsToUnits(walletAtoms(baseMint), market.base);
  const maxQuote = atomsToUnits(walletAtoms(quoteMint), market.quote);

  // A held position can deposit (top off) or withdraw; the two forms are
  // mutually exclusive, chosen from a dropdown. A fresh position only deposits,
  // so a full withdraw that closes the position drops back to the deposit form
  // (and a later top-off doesn't snap back to withdraw).
  const [mode, setMode] = useState<"deposit" | "withdraw">("deposit");
  if (!position && mode !== "deposit") setMode("deposit");
  // Bumped on each confirmed withdraw, remounting the withdraw form so the
  // fraction just redeemed can't be sent again by a second click.
  const [withdrawEpoch, setWithdrawEpoch] = useState(0);
  // A finished transaction's result line clears once the user starts a new
  // amount, so it never sits under a form it doesn't describe.
  const clearTxResult = () => {
    if (tx.status === "success" || tx.status === "error") tx.reset();
  };

  const base = Number.parseFloat(baseAmount);
  const quote = Number.parseFloat(quoteAmount);
  // The sizing of the driven leg, and whether the wallet covers the basket it
  // pulls. A one-sided vault pulls nothing on its empty leg, so a basket needs
  // only one positive leg.
  const sized =
    activeLeg === null
      ? null
      : sizeLeg(activeLeg, activeLeg === "base" ? baseAmount : quoteAmount);
  const sizing = sized && "sizing" in sized ? sized.sizing : null;
  // A basket is checked against the wallet only once its balances are known:
  // while they load, or with no wallet, there is no shortfall to report, just
  // nothing to submit yet.
  const balancesKnown = owner !== null && balances.isReady;
  const shortOf =
    sizing && balancesKnown
      ? sizing.baseIn > walletAtoms(baseMint)
        ? market.base
        : sizing.quoteIn > walletAtoms(quoteMint)
          ? market.quote
          : null
      : null;
  const sizingError =
    (sized && "error" in sized ? sized.error : null) ??
    (shortOf ? `Not enough ${shortOf} in your wallet.` : null);
  const validBasket = sizing !== null && balancesKnown && shortOf === null;
  // The program's own gate and the surface's closed states, checked against
  // the connected wallet so a leader isn't offered a deposit into their own
  // vault; past them, a deposit still needs a wallet to come from.
  const closedReason =
    contributeClosedReason(market, vault, owner) ??
    (owner === null ? "Connect a wallet to deposit." : null);
  const depositBlocked = closedReason !== null;
  const depositLabel = position ? "Top off" : "Open position";

  // Quote-denominated value of the entered basket (the base leg marked at the
  // vault's reserve ratio). Tested against the deposit cap below — the cap is
  // in the same quote/share units, so this stays quote-denominated.
  const depositValue =
    (Number.isFinite(base) ? base : 0) * (ratio ?? 0) +
    (Number.isFinite(quote) ? quote : 0);
  // USD value of the basket, for display only ("Total Deposit" + per-leg ≈).
  // Each leg's quote-denominated value × the quote token's USD price, so the
  // legs and the total share one basis and read as real USD even when the
  // quote leg isn't a USD stable (e.g. GYEN). The base leg is priced at the
  // vault's reference price, not the basket ratio, which measures holdings.
  const quoteUsd = vault.quoteUsd;
  const baseUsdValue =
    (Number.isFinite(base) ? base : 0) *
    (vaultRefPrice(vault) ?? ratio ?? 0) *
    quoteUsd;
  const quoteUsdValue = (Number.isFinite(quote) ? quote : 0) * quoteUsd;
  const depositUsd = baseUsdValue + quoteUsdValue;
  // The largest deposit the vault can take before the new shares would dilute
  // the leader below their min_leader_share floor; null when the floor can't
  // bind. A basket past the cap would be rejected on-chain, so we block it and
  // warn rather than let the user submit a doomed deposit.
  const depositCap = maxOutsideDepositValue(vault);
  const floorBreached =
    depositCap !== null && validBasket && depositValue > depositCap;
  const floorPct = Math.round(leaderFloorFraction(vault) * 100);

  // Send the deposit (re-sized against a fresh read of the market) and stay
  // open to report the result.
  const submitDeposit = async () => {
    if (!validBasket || depositBlocked || floorBreached || busy) return;
    if (activeLeg === null) return;
    const symbol = activeLeg === "base" ? market.base : market.quote;
    const value = activeLeg === "base" ? baseAmount : quoteAmount;
    const confirmed = await tx.execute({
      kind: "deposit",
      leg: activeLeg,
      amountIn: parseAmountToBase(value, stablecoinDecimals(symbol)),
      walletAtoms: {
        base: walletAtoms(baseMint),
        quote: walletAtoms(quoteMint),
      },
    });
    // Clear the form once it lands, so the confirmed amounts can't be
    // resubmitted by a second click; a failure keeps them for a retry.
    if (confirmed) {
      setBaseAmount("");
      setQuoteAmount("");
      setActiveLeg(null);
    }
  };
  const submitWithdraw = async (bps: number) => {
    if (await tx.execute({ kind: "withdraw", bps })) {
      setWithdrawEpoch((n) => n + 1);
    }
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
            <Dialog.Close
              aria-label="Close"
              className="rounded-md p-1 text-muted-fg transition-colors hover:bg-muted hover:text-foreground"
            >
              <X size={14} />
            </Dialog.Close>
          </div>

          <div className="flex flex-col gap-4 p-5">
            {/* The vault's name: one line, as text (React escapes it), never
                a link. Renders nothing while `name` is null (unset). */}
            {vault.name && (
              <p className="truncate font-medium text-foreground text-sm">
                {vault.name}
              </p>
            )}
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
                title={`View leader on ${EXPLORER_NAME}`}
                aria-label={`View leader on ${EXPLORER_NAME}`}
                className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
              >
                <ExternalLink size={12} />
              </a>
              {/* The vault's public link (by market and seq), for sharing or
                  a QR code. */}
              <span className="ml-auto flex items-center gap-1">
                <span>Link</span>
                <CopyButton
                  value={`${window.location.origin}${vaultLinkHref(market.base, market.quote, chain.seq)}`}
                  label="vault link"
                />
              </span>
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
                  {/* This deposit's share of the vault once it lands, from
                      the SDK's own sizing. A share, never a share count: the
                      surface carries no share math. */}
                  {sizing && (
                    <div className="flex items-center justify-between gap-2 font-mono text-[10px] text-muted-fg tabular-nums">
                      <span>Your share of the vault ≈</span>
                      <NumberFlow
                        value={
                          Number(sizing.sharesOut) /
                          Number(chain.totalShares + sizing.sharesOut)
                        }
                        format={FORMATS.percent}
                      />
                    </div>
                  )}
                </div>

                {sizingError && !depositBlocked && (
                  <p className="rounded-md border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-amber-300 text-xs">
                    {sizingError}
                  </p>
                )}
                {floorBreached && !depositBlocked && (
                  <p className="rounded-md border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-amber-300 text-xs">
                    Too large. This would push the leader below their {floorPct}
                    % minimum stake, so the transaction would be rejected. Lower
                    the amount.
                  </p>
                )}
                <button
                  type="button"
                  onClick={() => void submitDeposit()}
                  disabled={
                    !validBasket || depositBlocked || floorBreached || busy
                  }
                  className="h-10 rounded-md bg-accent px-3 font-medium text-background text-sm transition-colors hover:opacity-90 disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg"
                >
                  {depositLabel}
                </button>
                {closedReason && (
                  <p className="text-center text-muted-fg text-xs">
                    {closedReason}
                  </p>
                )}
                <ContributeTerms vault={vault} />
              </>
            ) : (
              position && (
                <WithdrawSection
                  key={withdrawEpoch}
                  market={market}
                  vault={vault}
                  position={position}
                  busy={busy}
                  onSubmit={(bps) => void submitWithdraw(bps)}
                  onEdit={clearTxResult}
                />
              )
            )}
            <TxStatusLine tx={tx} />
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
