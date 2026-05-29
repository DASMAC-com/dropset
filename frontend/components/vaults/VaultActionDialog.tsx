"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { useState } from "react";
import { ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import {
  type Vault,
  type VaultMarket,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { emit } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";

export type VaultActionMode = "deposit" | "withdraw";

// Format a derived pro-rata amount for display in the paired input: trim to 6
// decimals, drop trailing zeros, no grouping (keeps it copy/paste friendly).
const formatDerived = (n: number): string =>
  Number.isFinite(n) ? String(Number(n.toFixed(6))) : "";

// Deposit / withdraw modal for a single vault. Both legs of the basket are
// shown: editing the base (or quote) amount auto-fills the other pro-rata to
// the vault's current reserve ratio, so liquidity is added/removed without
// moving the vault's price. The on-chain submit is gated until the
// OpenVault/Deposit/Withdraw instructions ship (see TODO(program) below).
export function VaultActionDialog({
  market,
  vault,
  connected,
  open,
  onOpenChange,
}: {
  market: VaultMarket;
  vault: Vault;
  connected: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  // A frozen vault is withdraw-only, so it opens on the Withdraw tab and the
  // Deposit tab is disabled below.
  const [mode, setMode] = useState<VaultActionMode>(
    vault.frozen ? "withdraw" : "deposit",
  );
  const [baseAmount, setBaseAmount] = useState("");
  const [quoteAmount, setQuoteAmount] = useState("");

  // Quote tokens per base token; null for an empty vault (no ratio to hold).
  const ratio = vaultReserveRatio(vault);

  const onBaseChange = (value: string) => {
    setBaseAmount(value);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setQuoteAmount(Number.isFinite(n) ? formatDerived(n * ratio) : "");
  };
  const onQuoteChange = (value: string) => {
    setQuoteAmount(value);
    if (ratio === null) return;
    const n = Number.parseFloat(value);
    setBaseAmount(Number.isFinite(n) ? formatDerived(n / ratio) : "");
  };

  const depositBlocked =
    mode === "deposit" && (vault.frozen || !vault.outsideDepositsApproved);
  const title = mode === "deposit" ? "Deposit" : "Withdraw";

  // Primary CTA: connect first if disconnected, otherwise the action is
  // disabled until the program is live on-chain (or deposits aren't approved).
  const cta = !connected
    ? { label: "Connect wallet", disabled: false }
    : { label: title, disabled: true };

  const onPrimary = () => {
    if (!connected) {
      emit("openWalletModal");
      return;
    }
    // TODO(program): build + send the Deposit/Withdraw transaction once the
    // vault program is deployed. The signed amounts are the paired basket
    // derived above; deposits mint vault shares, withdrawals redeem them.
    // Until then this button stays disabled.
  };

  const amountField = (
    side: "base" | "quote",
    symbol: string,
    value: string,
    onChange: (v: string) => void,
  ) => (
    <label className="flex flex-col gap-1.5">
      <span className="text-muted-fg text-xs">
        {side === "base" ? "Base" : "Quote"} · {symbol}
      </span>
      <input
        type="text"
        inputMode="decimal"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="0.00"
        className="h-10 rounded-md border border-border bg-muted px-3 font-mono text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent"
      />
    </label>
  );

  const segBtn = (m: VaultActionMode, label: string) => {
    const disabled = m === "deposit" && vault.frozen;
    return (
      <button
        type="button"
        onClick={() => setMode(m)}
        disabled={disabled}
        title={disabled ? "This vault is frozen — withdrawals only" : undefined}
        className={`flex-1 rounded-md px-3 py-1.5 font-medium text-sm transition-colors disabled:cursor-not-allowed disabled:text-muted-fg/50 ${
          mode === m
            ? "bg-background text-foreground shadow-sm"
            : "text-muted-fg hover:text-foreground"
        }`}
      >
        {label}
      </button>
    );
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
              {market.label}
            </Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-muted-fg transition-colors hover:bg-muted hover:text-foreground">
              <X size={14} />
            </Dialog.Close>
          </div>

          <div className="flex flex-col gap-4 p-5">
            <div className="flex gap-1 rounded-lg bg-muted p-1">
              {segBtn("deposit", "Deposit")}
              {segBtn("withdraw", "Withdraw")}
            </div>

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

            <div className="flex items-center justify-between rounded-md border border-border bg-muted px-3 py-2 text-xs">
              <span className="text-muted-fg">Your deposit</span>
              <span className="font-mono text-foreground">
                {connected ? "$0.00" : "—"}
              </span>
            </div>

            {amountField("base", market.base, baseAmount, onBaseChange)}
            {amountField("quote", market.quote, quoteAmount, onQuoteChange)}

            <p className="text-muted-fg text-xs">
              {ratio === null
                ? "This vault has no reserves yet, so amounts aren't linked."
                : `Amounts fill pro-rata to the vault's reserves — set ${market.base} or ${market.quote} and the other follows.`}
            </p>

            <button
              type="button"
              onClick={onPrimary}
              disabled={cta.disabled}
              className="h-10 rounded-md bg-accent px-3 font-medium text-background text-sm transition-colors hover:opacity-90 disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg"
            >
              {cta.label}
            </button>

            {connected && (
              <p className="text-center text-muted-fg text-xs">
                {depositBlocked
                  ? "Outside deposits aren't approved for this vault yet."
                  : `${mode === "deposit" ? "Deposits" : "Withdrawals"} open once the vault program is live on-chain.`}
              </p>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
