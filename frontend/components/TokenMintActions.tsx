"use client";

import { useState } from "react";
import { explorerAddressUrl } from "@/lib/explorer";
import { Check, ExternalLink } from "./icons";

// Shared trailing chrome for a stablecoin row: a truncated mint button that
// copies the full address on click, followed by a Solana Explorer link.
// Used by both the swap-page token picker and the on-map country picker so
// behavior + styling stay aligned.
export function TokenMintActions({
  symbol,
  mint,
}: {
  symbol: string;
  mint: string;
}) {
  const [copied, setCopied] = useState(false);

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(mint);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // clipboard API unavailable — silently ignore
    }
  };

  return (
    <>
      <button
        type="button"
        onClick={onCopy}
        title={copied ? "Copied!" : "Copy mint address"}
        className="flex shrink-0 items-center gap-1 rounded px-1.5 py-1 font-mono text-muted-fg text-xs hover:bg-muted hover:text-accent"
      >
        {copied ? (
          <>
            <Check size={10} />
            copied
          </>
        ) : (
          <>
            {mint.slice(0, 4)}…{mint.slice(-4)}
          </>
        )}
      </button>
      <a
        href={explorerAddressUrl(mint)}
        target="_blank"
        rel="noopener noreferrer"
        title={`View ${symbol} on Solana Explorer`}
        className="flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
      >
        <ExternalLink size={12} />
      </a>
    </>
  );
}
