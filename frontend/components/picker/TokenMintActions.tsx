"use client";

import { ExternalLink } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { explorerTokenUrl } from "@/lib/explorer";

// Shared trailing chrome for a stablecoin row: a truncated mint label, the
// shared `<CopyButton>` (which renders its own inline "Copied:" popover), and
// a Solscan link. Used by both the swap-page token picker and the on-map
// country picker so behavior + styling stay aligned. The mint label is
// intentionally a non-interactive `<span>` — copy lives in its own icon.
export function TokenMintActions({
  symbol,
  mint,
}: {
  symbol: string;
  mint: string;
}) {
  return (
    <>
      <span title={mint} className="shrink-0 font-mono text-muted-fg text-xs">
        {mint.slice(0, 4)}…{mint.slice(-4)}
      </span>
      <CopyButton value={mint} label="mint address" />
      <a
        href={explorerTokenUrl(mint)}
        target="_blank"
        rel="noopener noreferrer"
        title={`View ${symbol} on Solscan`}
        className="flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
      >
        <ExternalLink size={12} />
      </a>
    </>
  );
}
