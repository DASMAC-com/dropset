"use client";

import { QRCodeSVG } from "qrcode.react";
import { useState } from "react";
import { KNOWN_WALLETS } from "@/lib/data/wallets";
import { inWalletBrowseUrl } from "@/lib/wallet/mobile";

// Only the wallets that publish an in-app browser link, in curated order, so
// Phantom leads and is the default.
const BROWSE_WALLETS = KNOWN_WALLETS.filter((w) => w.browse);

/**
 * The in-person fast path: a QR code on the desktop picker that opens this
 * exact page inside a wallet's in-app browser on whoever scans it. Showing
 * the swap to someone in person otherwise means talking them through finding
 * the in-app browser and typing the URL, which is slow for a site the wallet
 * doesn't list. The phone camera resolves the universal link straight into
 * the wallet (or its download page when the app is missing). Client-only:
 * render it only after hydration, since it reads `window.location`.
 */
export function PhoneHandoff() {
  const [selectedKey, setSelectedKey] = useState(BROWSE_WALLETS[0]?.key);
  const wallet = BROWSE_WALLETS.find((w) => w.key === selectedKey);
  const url = wallet
    ? inWalletBrowseUrl(
        wallet.browse,
        window.location.href,
        window.location.origin,
      )
    : undefined;
  if (!wallet || !url) return null;

  return (
    <div className="flex flex-col items-center gap-3 border-border border-t px-5 py-4">
      <div className="text-center text-muted-fg text-xs">
        On your phone? Scan to open in {wallet.name}.
      </div>
      {/* White quiet zone: scanners need dark-on-light with a margin, whatever
          the app theme. */}
      <div className="rounded-lg bg-white p-3">
        <QRCodeSVG value={url} size={176} bgColor="#ffffff" fgColor="#000000" />
      </div>
      <div className="flex gap-1">
        {BROWSE_WALLETS.map((w) => (
          <button
            key={w.key}
            type="button"
            onClick={() => setSelectedKey(w.key)}
            aria-pressed={w.key === selectedKey}
            className={`rounded-md px-2 py-1 text-xs transition-colors ${
              w.key === selectedKey
                ? "bg-muted text-foreground"
                : "text-muted-fg hover:bg-muted"
            }`}
          >
            {w.name}
          </button>
        ))}
      </div>
    </div>
  );
}
