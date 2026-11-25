"use client";

import NumberFlow from "@number-flow/react";
import * as Dialog from "@radix-ui/react-dialog";
import { useMemo, useState } from "react";
import { ChevronDown, ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import type { VaultPosition } from "@/lib/data/positions";
import {
  type Vault,
  type VaultMarket,
  vaultNumberLabel,
} from "@/lib/data/vaults";
import { EXPLORER_NAME, explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";
import { useClusterStatus } from "@/lib/hooks/useClusterStatus";
import { useVaultDepositForm } from "@/lib/hooks/useVaultDepositForm";
import { useVaultTx } from "@/lib/hooks/useVaultTx";
import { DIALOG_CONTENT_POSITION, DIALOG_OVERLAY_CLASS } from "@/lib/ui/dialog";
import { vaultLinkHref } from "@/lib/ui/swapUrl";
import { ContributeTerms } from "./ContributeTerms";
import { DepositLeg } from "./DepositLeg";
import { PositionDetail } from "./PositionDetail";
import { atomsToUnits } from "./parts";
import { TxStatusLine } from "./TxStatusLine";
import { WithdrawSection } from "./WithdrawSection";

// Manage a single vault position. With no position the user opens one; with a
// position they see its PnL detail and can top off or withdraw any fraction of
// the basket (take-profit). The buttons build, sign and send the Deposit /
// Withdraw instruction (lib/eclob/vaultTx.ts) and report the share balance
// read back from chain. The deposit form's sizing and validation rules live in
// lib/vaults/depositForm.ts, its input state in useVaultDepositForm.
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
  const chain = vault.chain;
  const form = useVaultDepositForm(market, vault, position);
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

  // Editing a leg also clears a finished transaction's result line.
  const onBaseChange = (value: string) => {
    form.setLeg("base", value);
    clearTxResult();
  };
  const onQuoteChange = (value: string) => {
    form.setLeg("quote", value);
    clearTxResult();
  };
  const maxBase = atomsToUnits(form.walletAtoms.base, market.base);
  const maxQuote = atomsToUnits(form.walletAtoms.quote, market.quote);

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

  const { sizing, depositBlocked, floorBreached, validBasket } = form;

  // Send the deposit (re-sized against a fresh read of the market) and stay
  // open to report the result.
  const submitDeposit = async () => {
    const request = form.request();
    if (request === null || busy) return;
    // Clear the form once it lands, so the confirmed amounts can't be
    // resubmitted by a second click; a failure keeps them for a retry.
    if (await tx.execute(request)) form.reset();
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
                    value={form.baseAmount}
                    onChange={onBaseChange}
                    autoFilled={form.activeLeg === "quote"}
                    max={maxBase}
                    usdValue={form.legs.base.usd}
                    disabled={depositBlocked}
                  />
                  <DepositLeg
                    symbol={market.quote}
                    otherSymbol={market.base}
                    value={form.quoteAmount}
                    onChange={onQuoteChange}
                    autoFilled={form.activeLeg === "base"}
                    max={maxQuote}
                    usdValue={form.legs.quote.usd}
                    disabled={depositBlocked}
                  />
                  {/* Total of both legs in USD, display only; the cap check
                      uses the quote-denominated basket value. */}
                  <div className="flex items-center justify-between gap-2 border-border border-t pt-2">
                    <span className="font-medium text-foreground text-sm">
                      Total Deposit
                    </span>
                    <span className="font-mono font-semibold text-foreground text-sm tabular-nums">
                      <NumberFlow value={form.usd} format={FORMATS.usd} />
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

                {form.error && !depositBlocked && (
                  <p className="rounded-md border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-amber-300 text-xs">
                    {form.error}
                  </p>
                )}
                {floorBreached && !depositBlocked && (
                  <p className="rounded-md border border-amber-400/40 bg-amber-400/10 px-3 py-2 text-amber-300 text-xs">
                    Too large. This would push the leader below their{" "}
                    {form.floorPct}% minimum stake, so the transaction would be
                    rejected. Lower the amount.
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
                  {form.label}
                </button>
                {form.closedReason && (
                  <p className="text-center text-muted-fg text-xs">
                    {form.closedReason}
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
