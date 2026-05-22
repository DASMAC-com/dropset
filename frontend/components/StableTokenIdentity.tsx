"use client";

import {
  type Stablecoin,
  shortenMint,
  tokenIconUrl,
  truncateTokenName,
} from "@/lib/currencies";
import { explorerTokenUrl } from "@/lib/explorer";
import { useLiquidityLookup } from "@/lib/useUsdQuote";
import { CopyButton } from "./CopyButton";
import { TokenInfoLink } from "./TokenInfoLink";

// Shared stablecoin-row identity block used by the dropdown TokenPicker and
// the globe's country-anchored picker. Renders icon + symbol (with optional
// "Illiquid" badge when Jupiter has no reliable USD price) + secondary name.
// Returns a fragment so each parent supplies its own flex container and
// action buttons.
export function StableTokenIdentity({
  s,
  symbolClassName,
}: {
  s: Stablecoin;
  symbolClassName?: string;
}) {
  const liquidityOf = useLiquidityLookup();
  const illiquid = liquidityOf(s.mint) === "illiquid";
  return (
    <>
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
      <img
        src={tokenIconUrl(s.symbol)}
        alt=""
        aria-hidden
        width={20}
        height={20}
        className="h-5 w-5 shrink-0 rounded-full"
      />
      <span className="flex min-w-0 flex-1 flex-col text-sm">
        <span className="flex items-center gap-1.5">
          <span className={`font-mono ${symbolClassName ?? ""}`}>
            {s.symbol}
          </span>
          {illiquid && (
            <span
              className="rounded border border-border bg-background px-1.5 py-0.5 text-[10px] text-muted-fg uppercase tracking-wide"
              title={`Market data unavailable for ${s.symbol}`}
            >
              Illiquid
            </span>
          )}
        </span>
        {/* Secondary line: truncating name + always-visible Solscan link on
            the shortened mint. Layout uses a nested flex with `min-w-0
            truncate` on the name and `shrink-0` on the bullet + mint so long
            names (e.g. "World Liberty Financial USD") collapse with an
            ellipsis while the mint stays fully readable. The mint link
            stopPropagation-s so clicking it in the dropdown picker (where
            this component is wrapped in a row-select button) doesn't also
            trigger the row select. Nesting `<a>` in `<button>` is technically
            invalid HTML but works in every major browser and is much smaller
            than restructuring the picker row. */}
        <span className="flex min-w-0 items-baseline gap-1 text-muted-fg text-xs">
          <span className="min-w-0 truncate">{truncateTokenName(s.name)}</span>
          <span className="shrink-0">·</span>
          <a
            href={explorerTokenUrl(s.mint)}
            target="_blank"
            rel="noreferrer"
            onClick={(e) => e.stopPropagation()}
            title={`View ${s.symbol} on Solscan`}
            className="shrink-0 font-mono hover:text-foreground hover:underline"
          >
            {shortenMint(s.mint)}
          </a>
          <CopyButton value={s.mint} label="mint address" />
          <TokenInfoLink symbol={s.symbol} />
        </span>
      </span>
    </>
  );
}
