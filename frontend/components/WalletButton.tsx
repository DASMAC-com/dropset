"use client";

import * as Dialog from "@radix-ui/react-dialog";
import * as Popover from "@radix-ui/react-popover";
import { useWalletConnection, useWalletModalState } from "@solana/react-hooks";
import Image from "next/image";
import { useState } from "react";
import { useAppEvent } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";
import { Check, ChevronDown, Copy, ExternalLink, X } from "./icons";

export function WalletButton() {
  const { connected, wallet, status, currentConnector } = useWalletConnection();
  const modal = useWalletModalState({ closeOnConnect: true });
  const [copied, setCopied] = useState(false);

  // SwapPanel's CTA (and any other surface) can request the modal via this event.
  // Each useWalletModalState() call owns its own isOpen state — they don't share —
  // so we route external "open the picker" requests through the event bus into
  // this single hook instance.
  useAppEvent("openWalletModal", () => modal.open());
  useAppEvent("toggleWallet", () =>
    connected ? modal.disconnect() : modal.open(),
  );

  if (!modal.isReady) {
    return <div className="h-9 w-32 animate-pulse rounded-md bg-muted" />;
  }

  const picker = (
    <Dialog.Root
      open={modal.isOpen}
      onOpenChange={(open) => (open ? modal.open() : modal.close())}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/50" />
        <Dialog.Content
          aria-describedby={undefined}
          className="-translate-x-1/2 -translate-y-1/2 fixed top-1/2 left-1/2 z-50 w-80 rounded-2xl border border-border bg-background shadow-xl"
        >
          <div className="flex items-center justify-between border-border border-b px-5 py-4">
            <Dialog.Title className="font-semibold text-foreground">
              Connect a wallet
            </Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-muted-fg transition-colors hover:bg-muted hover:text-foreground">
              <X size={14} />
            </Dialog.Close>
          </div>

          <div className="p-3">
            {modal.connectors.length === 0 && (
              <p className="px-3 py-6 text-center text-muted-fg text-sm">
                No wallets detected.
              </p>
            )}

            {modal.connectors.map((connector) => (
              <button
                key={connector.id}
                type="button"
                disabled={modal.status === "connecting"}
                onClick={() => modal.connect(connector.id)}
                className="flex w-full items-center gap-3 rounded-lg px-3 py-3 text-left text-sm transition-colors hover:bg-muted disabled:opacity-50"
              >
                {connector.icon ? (
                  <Image
                    src={connector.icon}
                    alt=""
                    width={32}
                    height={32}
                    className="h-8 w-8 rounded-lg"
                    unoptimized
                  />
                ) : (
                  <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-muted font-bold text-muted-fg text-xs">
                    {connector.name.charAt(0)}
                  </div>
                )}
                <span className="flex-1 font-medium text-foreground">
                  {connector.name}
                </span>
                {connector.ready && (
                  <span className="text-accent-buy text-xs">Detected</span>
                )}
              </button>
            ))}

            {modal.status === "connecting" && (
              <div className="px-3 py-3 text-center text-muted-fg text-xs">
                Connecting...
              </div>
            )}
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );

  if (!connected || !wallet) {
    return (
      <>
        <button
          type="button"
          onClick={() => modal.open()}
          disabled={status === "connecting"}
          className="inline-flex h-9 items-center rounded-md bg-accent-buy px-3 font-medium text-background text-sm transition-colors hover:bg-accent-buy-hover disabled:cursor-not-allowed disabled:bg-muted disabled:text-muted-fg"
        >
          {status === "connecting" ? "Connecting…" : "Connect Wallet"}
        </button>
        {picker}
      </>
    );
  }

  const addr = wallet.account.address;
  const short = `${addr.slice(0, 4)}...${addr.slice(-4)}`;

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(addr);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // clipboard blocked (e.g. insecure context); silently ignore
    }
  };

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <button
          type="button"
          className="inline-flex h-9 items-center gap-2 rounded-md border border-muted-fg/40 bg-foreground/[0.07] px-3 font-medium text-foreground text-sm transition-colors hover:border-muted-fg/70 hover:bg-foreground/[0.12]"
        >
          {currentConnector?.icon && (
            <Image
              src={currentConnector.icon}
              alt=""
              width={16}
              height={16}
              className="h-4 w-4 rounded-sm"
              unoptimized
            />
          )}
          <span className="font-mono tabular-nums [font-variant-ligatures:none]">
            {short}
          </span>
          <ChevronDown size={14} className="text-muted-fg" />
        </button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="end"
          sideOffset={8}
          className="z-50 w-48 rounded-xl border border-border bg-background p-1 shadow-lg"
        >
          <button
            type="button"
            onClick={handleCopy}
            className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-foreground text-sm transition-colors hover:bg-muted"
          >
            {copied ? (
              <Check size={14} className="text-accent-buy" />
            ) : (
              <Copy size={14} />
            )}
            {copied ? "Copied" : "Copy address"}
          </button>
          <a
            href={explorerAddressUrl(addr)}
            target="_blank"
            rel="noopener noreferrer"
            className="flex w-full items-center gap-2 rounded-md px-3 py-2 text-left text-foreground text-sm no-underline transition-colors hover:bg-muted"
          >
            <ExternalLink size={14} />
            Open on Solscan
          </a>
          <button
            type="button"
            onClick={() => modal.disconnect()}
            className="w-full rounded-md px-3 py-2 text-left text-red-500 text-sm transition-colors hover:bg-muted"
          >
            Disconnect
          </button>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
