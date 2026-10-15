"use client";

import { QRCodeSVG } from "qrcode.react";
import { inWalletBrowseUrl } from "@/lib/wallet/mobile";

/**
 * The in-person fast path: a QR code, revealed from a desktop picker row,
 * that opens this exact page inside that wallet's in-app browser on whoever
 * scans it. Showing the swap to someone in person otherwise means talking
 * them through finding the in-app browser and typing the URL, which is slow
 * for a site the wallet doesn't list. The phone camera resolves the universal
 * link straight into the wallet (or its download page when the app is
 * missing). Client-only: render it only after hydration, since it reads
 * `window.location`.
 */
export function PhoneHandoff({
  browse,
  name,
}: {
  browse: string | undefined;
  name: string;
}) {
  const url = inWalletBrowseUrl(
    browse,
    window.location.href,
    window.location.origin,
  );
  if (!url) return null;

  return (
    <div className="flex flex-col items-center gap-3 px-3 pt-1 pb-3">
      <div className="text-center text-muted-fg text-xs">
        Scan with a phone camera to open this page in {name}.
      </div>
      {/* White quiet zone: scanners need dark-on-light with a margin, whatever
          the app theme. */}
      <div className="rounded-lg bg-white p-3">
        <QRCodeSVG value={url} size={176} bgColor="#ffffff" fgColor="#000000" />
      </div>
    </div>
  );
}
