"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { useWallet } from "@solana/react-hooks";
import { useMemo, useState } from "react";
import { ChevronDown, ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  shortenMint,
  stablecoinDecimals,
  stablecoinMint,
} from "@/lib/data/currencies";
import type { VaultPosition } from "@/lib/data/positions";
import {
  contributeClosedReason,
  leaderFloorFraction,
  maxOutsideDepositValue,
  type Vault,
  type VaultMarket,
  vaultNumberLabel,
  vaultRefPrice,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import {
  type DepositLeg as DepositLegKind,
  type DepositSizing,
  depositSizingError,
  sizeDeposit,
  VAULT_SLIPPAGE_BPS,
} from "@/lib/eclob/vaultTx";
import { EXPLORER_NAME, explorerAddressUrl } from "@/lib/explorer";
import { formatBaseAmount, parseAmountToBase } from "@/lib/format/balance";
import { FORMATS } from "@/lib/format/formats";
import { useAllBalances } from "@/lib/hooks/useAllBalances";
import { useClusterStatus } from "@/lib/hooks/useClusterStatus";
import { useVaultTx } from "@/lib/hooks/useVaultTx";
import { DIALOG_CONTENT_POSITION, DIALOG_OVERLAY_CLASS } from "@/lib/ui/dialog";
import { vaultLinkHref } from "@/lib/ui/swapUrl";
import { ContributeTerms } from "./ContributeTerms";
import { DepositLeg } from "./DepositLeg";
import { PositionDetail } from "./PositionDetail";
import { atomsToUnits } from "./parts";
import { TxStatusLine } from "./TxStatusLine";
import { WithdrawSection } from "./WithdrawSection";

// Atoms → a whole-token input string at the token's full precision, the same
// shape DepositLeg's padToken gives a float.
const atomsToInput = (atoms: bigint, symbol: string): string => {
  const decimals = stablecoinDecimals(symbol);
  return formatBaseAmount(atoms, decimals, decimals);
};

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
  // "unconfirmed" counts: the transaction may still land, so the form stays
  // locked until the dialog is closed and reopened after checking it. So does
  // an endpoint not yet proved to serve the build's cluster: briefly while
  // the check runs, and for good on a mismatch (the wrong-network banner
  // says why).
  const clusterVerified = useClusterStatus() === "verified";
  const busy =
    tx.status === "preparing" ||
    tx.status === "signing" ||
    tx.status === "confirming" ||
    tx.status === "unconfirmed" ||
    !clusterVerified;

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
  // bind. A basket past the cap would be rejected onchain, so we block it and
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
    if (busy) return;
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
            {/* The vault's identity: its number, then its name when set — one
                line, as text (React escapes the name), never a link. */}
            <p className="flex items-baseline gap-2 font-medium text-foreground text-sm">
              <span className="shrink-0">{vaultNumberLabel(vault)}</span>
              {vault.name && <span className="truncate">{vault.name}</span>}
            </p>
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
                  {/* Total of both legs in USD (depositUsd), display only;
                      the cap check uses the quote-denominated depositValue. */}
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
