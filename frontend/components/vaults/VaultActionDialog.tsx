"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { useState } from "react";
import { ExternalLink, X } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import type { Vault, VaultMarket } from "@/lib/data/vaults";
import { emit } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";

export type VaultActionMode = "deposit" | "withdraw";

// Deposit / withdraw modal for a single vault. The on-chain submit is gated
// until the OpenVault/Deposit/Withdraw instructions ship — the real flow
// settles paired (base, quote) baskets per docs/architecture.md → Vault, so
// the tx builder is intentionally left as a seam (see TODO(program) below).
export function VaultActionDialog({
  market,
  vault,
  mode,
  connected,
  open,
  onOpenChange,
}: {
  market: VaultMarket;
  vault: Vault;
  mode: VaultActionMode;
  connected: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [amount, setAmount] = useState("");

  const title = mode === "deposit" ? "Deposit" : "Withdraw";
  const hasAmount = Number.parseFloat(amount) > 0;

  // Primary CTA: connect first if disconnected, otherwise the action is
  // disabled until the program is live on-chain.
  const cta = !connected
    ? { label: "Connect wallet", disabled: false }
    : { label: title, disabled: true };

  const onPrimary = () => {
    if (!connected) {
      emit("openWalletModal");
      return;
    }
    // TODO(program): build + send the Deposit/Withdraw transaction once the
    // vault program is deployed. Deposits settle a paired (base, quote)
    // basket sized to the vault's current share price; withdrawals redeem
    // shares back into both legs. Until then this button stays disabled.
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
              {title} · {market.label}
            </Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-muted-fg transition-colors hover:bg-muted hover:text-foreground">
              <X size={14} />
            </Dialog.Close>
          </div>

          <div className="flex flex-col gap-4 p-5">
            <div className="flex items-center gap-2 text-muted-fg text-xs">
              <span className="flex items-center gap-1">
                {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
                <img
                  src={market.baseFlagUrl}
                  alt=""
                  aria-hidden
                  width={16}
                  height={16}
                />
                {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
                <img
                  src={market.quoteFlagUrl}
                  alt=""
                  aria-hidden
                  width={16}
                  height={16}
                />
              </span>
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

            <label className="flex flex-col gap-1.5">
              <span className="text-muted-fg text-xs">Amount</span>
              <input
                type="text"
                inputMode="decimal"
                value={amount}
                onChange={(e) => setAmount(e.target.value)}
                placeholder="0.00"
                className="h-10 rounded-md border border-border bg-muted px-3 text-foreground text-sm outline-none placeholder:text-muted-fg focus:border-accent"
              />
            </label>

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
                {hasAmount ? `${title} of ${amount} ` : ""}
                {mode === "deposit" ? "Deposits" : "Withdrawals"} open once the
                vault program is live on-chain.
              </p>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
