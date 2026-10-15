"use client";

import * as Dialog from "@radix-ui/react-dialog";
import * as Popover from "@radix-ui/react-popover";
import { useWalletConnection, useWalletModalState } from "@solana/react-hooks";
import Image from "next/image";
import { type ReactNode, useEffect, useState } from "react";
import { createPortal } from "react-dom";
import {
  Check,
  ChevronDown,
  Copy,
  ExternalLink,
  Loader2,
  X,
} from "@/components/icons";
import { WalletIcon } from "@/components/ui/WalletIcon";
import { PhoneHandoff } from "@/components/wallet/PhoneHandoff";
import { COPY_FEEDBACK_DURATION_MS } from "@/lib/data/timings";
import { buildPickerWallets, type PickerWallet } from "@/lib/data/wallets";
import { emit, useAppEvent } from "@/lib/events";
import { EXPLORER_NAME, explorerAddressUrl } from "@/lib/explorer";
import { useWalletAccountWatch } from "@/lib/hooks/useWalletAccountWatch";
import { isMobile } from "@/lib/ua";
import { DIALOG_CONTENT_POSITION, DIALOG_OVERLAY_CLASS } from "@/lib/ui/dialog";
import {
  isMetaMaskExtensionPresent,
  useMetaMaskRegistrationReady,
} from "@/lib/wallet/metamask";
import { inWalletBrowseUrl, pickerRowAction } from "@/lib/wallet/mobile";

// 4 + 4 hex characters out of 64 is enough to disambiguate two wallets at
// a glance without taking up real estate in the header. Matches the
// convention Phantom / Backpack use in their own UIs.
const ADDR_ABBREV_LEN = 4;

export function WalletButton() {
  const { connected, wallet, status, currentConnector } = useWalletConnection();
  const modal = useWalletModalState({ closeOnConnect: true });
  const [copied, setCopied] = useState(false);

  // Gate MetaMask's "Detected" badge on a real extension rather than on our
  // relay-SDK registration. Re-checked whenever the picker opens so a wallet
  // installed mid-session is reflected without a reload.
  const [metamaskInstalled, setMetamaskInstalled] = useState(false);
  useEffect(() => {
    // Only re-check while the picker is open — that's the only time the badge
    // is visible, and it catches a wallet installed mid-session on reopen.
    if (modal.isOpen) setMetamaskInstalled(isMetaMaskExtensionPresent());
  }, [modal.isOpen]);

  // Desktop: which wallet row has its phone-handoff QR expanded, if any. One
  // at a time, and collapsed again whenever the picker closes so it reopens
  // as a plain wallet list.
  const [handoffId, setHandoffId] = useState<string | null>(null);
  useEffect(() => {
    if (!modal.isOpen) setHandoffId(null);
  }, [modal.isOpen]);

  // MetaMask's relay SDK initializes asynchronously (dynamic import +
  // createSolanaClient); on a cold first-of-day start the picker can render
  // before registration finishes, leaving a row whose connector isn't yet
  // live. Tapping it would race the SDK. We gate the row on this flag so the
  // user can't fire a connect the SDK isn't ready to honor.
  const metaMaskReady = useMetaMaskRegistrationReady();

  // True for the duration of a connect attempt — drives the blocking overlay
  // below. Tracked locally (not off `modal.status`) because an external SDK's
  // relay flow doesn't reliably hold the client in "connecting" while its modal
  // is open, which would let the overlay drop too early.
  const [connecting, setConnecting] = useState(false);

  // Drop the connection if the user switches accounts in their wallet
  // extension — the store doesn't track in-place account changes on its own.
  useWalletAccountWatch();

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

  const { detected, notDetected } = buildPickerWallets(
    modal.connectors,
    metamaskInstalled,
  );

  const renderIcon = (w: PickerWallet) => (
    <WalletIcon
      name={w.name}
      src={w.icon}
      fallbackSrc={w.iconFallback}
      size={32}
      className="h-8 w-8 rounded-lg"
    />
  );

  const mobile = isMobile();

  // On desktop, a wallet with an in-app browser link gets a small toggle
  // beside its row that expands that wallet's QR underneath, so someone else
  // can scan the page into their phone. The toggle is a sibling, not a
  // child: the row is itself a button or a link, which can't nest one. A row
  // with no link gets an empty slot of the toggle's width instead, so every
  // desktop row is the same width and the badges line up down the list.
  const renderEntry = (w: PickerWallet) => {
    const row = renderRow(w);
    if (mobile) return row;
    if (!w.browse) {
      return (
        <div key={w.id} className="flex items-center gap-1">
          {row}
          <span aria-hidden className="w-[30px] shrink-0" />
        </div>
      );
    }
    const open = handoffId === w.id;
    const hint = `Show a QR code to open this page in ${w.name} on a phone`;
    return (
      <div key={w.id}>
        <div className="flex items-center gap-1">
          {row}
          <button
            type="button"
            onClick={() => setHandoffId(open ? null : w.id)}
            aria-expanded={open}
            aria-label={hint}
            title={hint}
            className={`shrink-0 rounded-md p-2 transition-colors hover:bg-muted hover:text-foreground ${
              open ? "bg-muted text-foreground" : "text-muted-fg"
            }`}
          >
            <ExternalLink size={14} aria-hidden />
          </button>
        </div>
        {open && <PhoneHandoff browse={w.browse} name={w.name} />}
      </div>
    );
  };

  const renderRow = (w: PickerWallet) => {
    // MetaMask's relay connector lives in the Wallet Standard registry only
    // after `registerMetaMaskConnect()` resolves. Until then, hold the row
    // disabled so a fast first-of-day tap can't race the SDK's async init.
    // The curated `key="metamask"` matches both the extension and the relay,
    // and `buildPickerWallets` collapses them into a single row whose
    // connector is whichever the discovery loop saw first. If the extension
    // is present, that connector is sync-ready — no need to gate on relay
    // readiness, since either the row already holds the extension's
    // connector or a watcher-driven rebuild will swap it in shortly.
    const isMetaMaskRelayRow =
      w.key === "metamask" && w.connectorId !== undefined && !metamaskInstalled;
    const metaMaskPending = isMetaMaskRelayRow && !metaMaskReady;

    // "Detected" only for truly-present wallets. A wallet that's connectable
    // without being installed (MetaMask, via its relay) gets no badge — it
    // still connects on click, and "Not detected" would wrongly read as "must
    // install". Only wallets that genuinely require installation (a site link,
    // no connector) keep the amber "Not detected".
    //
    // While MetaMask's relay registration is in flight, swap the badge for a
    // small spinner so the disabled row has a visible reason for being
    // disabled.
    let badge: ReactNode = null;
    if (metaMaskPending) {
      badge = (
        <Loader2 size={14} className="animate-spin text-muted-fg" aria-hidden />
      );
    } else if (w.detected) {
      badge = <span className="text-accent-buy text-xs">Detected</span>;
    } else if (!w.connectorId) {
      badge = <span className="text-amber-400 text-xs">Not detected</span>;
    }

    const rowClass =
      "flex w-full items-center gap-3 rounded-lg px-3 py-3 text-left text-sm transition-colors hover:bg-muted";

    // A connectorId means we can connect right now (installed wallet, or
    // MetaMask's always-present relay).
    const action = pickerRowAction(w, mobile);
    const connectorId = w.connectorId;
    if (action === "connect" && connectorId) {
      return (
        <button
          key={w.id}
          type="button"
          disabled={modal.status === "connecting" || metaMaskPending}
          aria-busy={metaMaskPending || undefined}
          onClick={() => {
            // Close our picker first so the wallet's own modal (e.g. MetaMask's
            // relay QR dialog) owns the screen — leaving our Radix dialog open
            // underneath makes its focus-trap / scroll-lock fight MetaMask's
            // backdrop. Mark connecting (drives the blocking overlay) and catch
            // the rejection (user dismissed, transport timed out) — it's
            // surfaced via status, and an uncaught reject would otherwise become
            // an unhandledRejection.
            //
            // `walletConnecting` tells Providers a connect is in flight so it
            // won't swap the SolanaClient (reactive wallet discovery rebuilds
            // it on connector-set changes) mid-handshake: a swap mid-connect
            // lands the session on an orphaned client and leaves the header
            // stuck on "Connect Wallet" until a refresh. It is asserted before
            // `modal.connect` and released in the same `.finally` that clears
            // `connecting`, so the pair can't drift apart. An open picker alone
            // does not hold the guard, so a late-registering wallet reaches
            // the open picker.
            modal.close();
            emit("walletConnecting", true);
            setConnecting(true);
            void modal
              .connect(connectorId)
              .catch(() => {})
              .finally(() => {
                setConnecting(false);
                emit("walletConnecting", false);
              });
          }}
          className={`${rowClass} disabled:opacity-50`}
        >
          {renderIcon(w)}
          <span className="flex-1 font-medium text-foreground">{w.name}</span>
          {badge}
        </button>
      );
    }
    // Nothing injected, but the wallet has an in-app browser → on mobile,
    // reopen this page inside it, where the wallet injects itself and connects
    // like a desktop extension. This is the only route that works on iOS
    // Safari (no extensions, no Mobile Wallet Adapter), and Phantom leads the
    // curated order so it is the first thing an iPhone user sees. The
    // universal link falls through to the wallet's download page when the
    // app is missing. Same tab on purpose: the link hands off to the app.
    const browseUrl =
      action === "open-in"
        ? inWalletBrowseUrl(
            w.browse,
            window.location.href,
            window.location.origin,
          )
        : undefined;
    if (browseUrl) {
      return (
        <a key={w.id} href={browseUrl} className={`${rowClass} no-underline`}>
          {renderIcon(w)}
          <span className="flex-1">
            <span className="block font-medium text-foreground">
              Open in {w.name}
            </span>
            <span className="mt-0.5 block text-muted-fg text-xs">
              Continues in {w.name}'s in-app browser.
            </span>
          </span>
          <ExternalLink size={12} className="text-muted-fg" aria-hidden />
        </a>
      );
    }
    // Nothing to connect to → link out to the wallet's official site so the
    // user can install it (then it shows up as detected on next open). On
    // mobile this is now only the wallets with no in-app browser link
    // (MetaMask before its relay registers, Coinbase). The row reads as a
    // connect attempt to many users — tapping it can open the wallet's app
    // via a universal link but never sends a connect request — so on a mobile
    // UA we
    // surface an explicit "Install" affordance with an external-link glyph
    // and a hint line so the user knows the row is a download, not a connect.
    return (
      <a
        key={w.id}
        href={w.site}
        target="_blank"
        rel="noopener noreferrer"
        className={`${rowClass} no-underline`}
      >
        {renderIcon(w)}
        <span className="flex-1">
          <span className="block font-medium text-foreground">{w.name}</span>
          {mobile && (
            <span className="mt-0.5 block text-muted-fg text-xs">
              Install, then return — or open this site in {w.name}'s in-app
              browser.
            </span>
          )}
        </span>
        {mobile ? (
          <span className="flex items-center gap-1 text-amber-400 text-xs">
            Install
            <ExternalLink size={12} aria-hidden />
          </span>
        ) : (
          badge
        )}
      </a>
    );
  };

  const picker = (
    <Dialog.Root
      open={modal.isOpen}
      onOpenChange={(open) => (open ? modal.open() : modal.close())}
    >
      <Dialog.Portal>
        <Dialog.Overlay className={DIALOG_OVERLAY_CLASS} />
        <Dialog.Content
          aria-describedby={undefined}
          className={`${DIALOG_CONTENT_POSITION} flex w-[min(380px,calc(100vw-2rem))] flex-col overflow-y-auto rounded-2xl border border-border bg-background shadow-xl`}
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
            {detected.map(renderEntry)}

            {notDetected.length > 0 && (
              <>
                {detected.length > 0 && (
                  <div className="my-2 border-border border-t" />
                )}
                {notDetected.map(renderEntry)}
              </>
            )}

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

  // While a connect is in flight, an external SDK (MetaMask's relay QR dialog)
  // shows its own modal but doesn't reliably dim the page behind it. Our picker
  // is already closed, so add a dim layer ourselves. It sits below MetaMask's
  // own backdrop + content (z-index 99998/99999) and is portaled to <body> so
  // no ancestor stacking context can trap it. This is purely visual: MetaMask's
  // backdrop still sits on top, so clicking outside the dialog closes it.
  const connectingOverlay =
    connecting && typeof document !== "undefined"
      ? createPortal(
          <div
            className="fixed inset-0 z-[99990] bg-black/60 backdrop-blur-sm"
            aria-hidden="true"
          />,
          document.body,
        )
      : null;

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
        {connectingOverlay}
      </>
    );
  }

  const addr = wallet.account.address;
  const short = `${addr.slice(0, ADDR_ABBREV_LEN)}...${addr.slice(-ADDR_ABBREV_LEN)}`;

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(addr);
      setCopied(true);
      setTimeout(() => setCopied(false), COPY_FEEDBACK_DURATION_MS);
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
            Open on {EXPLORER_NAME}
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
