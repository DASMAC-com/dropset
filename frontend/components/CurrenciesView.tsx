// cspell:word colspanned
"use client";

import NumberFlow, { type Format } from "@number-flow/react";
import * as Popover from "@radix-ui/react-popover";
import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { CopyButton } from "@/components/CopyButton";
import {
  ArrowUpDown,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  HelpCircle,
  Info,
  Search,
  X,
} from "@/components/icons";
import {
  ALL_STABLECOIN_MINTS,
  CURRENCIES,
  currencyFlagUrl,
  currencyName,
  currencyStats,
  type IsoCurrencyCode,
  type Stablecoin,
  SUPPORTED,
  shortenMint,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { explorerTokenUrl } from "@/lib/explorer";
import { type Side, useSwapStore } from "@/lib/store";
import { flashBg, useFlashOnChange } from "@/lib/useFlashOnChange";
import {
  prefetchAllTokenInfo,
  REFRESH_INTERVAL_MS,
  sortByVolumeDesc,
  type TokenInfo,
  useInfoLookup,
  useTokenInfo,
} from "@/lib/useUsdQuote";

const COLSPAN = 9;

// <NumberFlow> format objects, hoisted to module scope so the same
// reference is reused across every row render. NumberFlow compares format
// by identity to decide whether to reset its animation, so passing a
// fresh object each render would kill the rolling-digit effect.
//
// Price uses max 6 decimals so sub-$1 stablecoin drift (e.g. $0.9987)
// stays legible while $1.00 still renders as "$1.00" (min 2). Trailing
// zeros above the minimum are trimmed by Intl.
const priceFormat: Format = {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 6,
};
const changeFormat: Format = {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
  // "+1.20" for gains, "-1.20" for losses, "0.00" for flat. Matches the
  // legacy formatPercent which prepended an explicit "+" sign.
  signDisplay: "exceptZero",
};
const compactUsdFormat: Format = {
  notation: "compact",
  style: "currency",
  currency: "USD",
  maximumFractionDigits: 2,
};
const compactCountFormat: Format = {
  notation: "compact",
  maximumFractionDigits: 1,
};

const isFiniteNumber = (n: unknown): n is number =>
  typeof n === "number" && Number.isFinite(n);

type SortKey = "volume24h" | "mcap" | "liquidity" | "holderCount";
type SortDir = "asc" | "desc";
type SortState = { key: SortKey; direction: SortDir } | null;

// Sort a stablecoin list by a numeric `TokenInfo` field. Tokens with no
// reported value sink to the bottom and retain their input order (Array.sort
// is stable in ES2019+), so the upstream JSON order is the implicit fallback
// for ties and nulls.
const sortStablesByMetric = <T extends { mint: string }>(
  list: T[],
  key: SortKey,
  direction: SortDir,
  lookup: (mint: string) => TokenInfo | null,
): T[] =>
  list.slice().sort((a, b) => {
    const va = lookup(a.mint)?.[key] ?? -1;
    const vb = lookup(b.mint)?.[key] ?? -1;
    return direction === "desc" ? vb - va : va - vb;
  });

// A currency group's rank is taken from its highest-ranked stablecoin on the
// active metric — so USD floats to the top when sorting by volume because
// USDC dominates, even if some EUR stable would individually outrank a long
// PYUSD-style tail. Groups with no data on this metric stay at the bottom.
const groupScore = <T extends { mint: string }>(
  stables: T[],
  key: SortKey,
  lookup: (mint: string) => TokenInfo | null,
): number => {
  let max = Number.NEGATIVE_INFINITY;
  for (const s of stables) {
    const v = lookup(s.mint)?.[key];
    if (typeof v === "number" && v > max) max = v;
  }
  return Number.isFinite(max) ? max : -1;
};

// Cache of dominant color (RGB triplet) computed from a flag SVG rasterized to
// a canvas. Module-level so it persists across re-renders / search filters.
type Rgb = [number, number, number];
const flagColorCache = new Map<string, Rgb | null>();

const sampleDominantColor = (
  ctx: CanvasRenderingContext2D,
  size: number,
): Rgb | null => {
  let r = 0;
  let g = 0;
  let b = 0;
  let n = 0;
  const { data } = ctx.getImageData(0, 0, size, size);
  for (let i = 0; i < data.length; i += 4) {
    const pa = data[i + 3];
    if (pa < 200) continue;
    const pr = data[i];
    const pg = data[i + 1];
    const pb = data[i + 2];
    const max = Math.max(pr, pg, pb);
    const min = Math.min(pr, pg, pb);
    const sat = max === 0 ? 0 : (max - min) / max;
    // Drop near-grey, very dark, and very bright pixels — keeps the
    // saturated brand color and avoids skewing toward white/black/grey bands.
    if (sat < 0.3) continue;
    if (max < 60 || max > 245) continue;
    r += pr;
    g += pg;
    b += pb;
    n++;
  }
  if (n === 0) return null;
  return [(r / n) | 0, (g / n) | 0, (b / n) | 0];
};

const computeFlagColor = (url: string): Promise<Rgb | null> => {
  if (typeof document === "undefined") return Promise.resolve(null);
  return new Promise((resolve) => {
    const img = new Image();
    img.onload = () => {
      const size = 24;
      const canvas = document.createElement("canvas");
      canvas.width = size;
      canvas.height = size;
      const ctx = canvas.getContext("2d", { willReadFrequently: true });
      if (!ctx) return resolve(null);
      ctx.clearRect(0, 0, size, size);
      ctx.drawImage(img, 0, 0, size, size);
      resolve(sampleDominantColor(ctx, size));
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
};

const useFlagColor = (code: IsoCurrencyCode, url: string): Rgb | null => {
  const [color, setColor] = useState<Rgb | null>(() =>
    flagColorCache.has(code) ? (flagColorCache.get(code) ?? null) : null,
  );
  useEffect(() => {
    if (flagColorCache.has(code)) return;
    let cancelled = false;
    computeFlagColor(url).then((c) => {
      if (cancelled) return;
      flagColorCache.set(code, c);
      setColor(c);
    });
    return () => {
      cancelled = true;
    };
  }, [code, url]);
  return color;
};

const xHref = (handle: string) => `https://x.com/${handle}`;

// Strict mode (driven by `?symbol=<SYM>` from the picker's `?` link) matches
// only on exact symbol equality so EURC doesn't surface EURCV alongside it.
// Fuzzy mode (the default) keeps the existing substring search across symbol,
// name, mint, currency code, and issuer.
const matches = (
  s: Stablecoin,
  code: IsoCurrencyCode,
  q: string,
  strict: boolean,
): boolean => {
  if (!q) return true;
  if (strict) return s.symbol.toLowerCase() === q;
  return (
    s.symbol.toLowerCase().includes(q) ||
    s.name.toLowerCase().includes(q) ||
    s.mint.toLowerCase().includes(q) ||
    code.toLowerCase().includes(q) ||
    currencyName(code).toLowerCase().includes(q) ||
    s.issuer.name.some((n) => n.toLowerCase().includes(q))
  );
};

function SortableHeader({
  sortKey,
  label,
  sort,
  onToggle,
}: {
  sortKey: SortKey;
  label: string;
  sort: SortState;
  onToggle: (key: SortKey) => void;
}) {
  const active = sort?.key === sortKey;
  const Icon = !active
    ? ArrowUpDown
    : sort.direction === "desc"
      ? ChevronDown
      : ChevronUp;
  return (
    <th className="sticky top-14 z-20 border-border border-r bg-muted p-0 last:border-r-0">
      <button
        type="button"
        onClick={() => onToggle(sortKey)}
        className={`flex w-full cursor-pointer select-none items-center justify-end gap-1 px-3 py-2 text-right font-medium outline-none transition-colors focus:outline-none focus-visible:outline-none ${active ? "text-foreground" : "text-muted-fg hover:text-foreground"}`}
      >
        {label}
        <Icon size={12} />
      </button>
    </th>
  );
}

function CurrencyHeaderRow({ code }: { code: IsoCurrencyCode }) {
  const url = currencyFlagUrl(code);
  const color = useFlagColor(code, url);
  // Render the tinted "open" bar via an inset box-shadow rather than a real
  // border. Real borders on this colspanned cell collapse with the `border-r`
  // separators in the row below, and the vertical lines visibly punch through
  // the colored bar at the corners. Box-shadow doesn't participate in
  // border-collapse, so the bar reads as a continuous unbroken line.
  const tdStyle = color
    ? {
        boxShadow: `inset 0 -2px 0 rgb(${color[0]} ${color[1]} ${color[2]} / 0.6)`,
      }
    : undefined;
  const chipStyle = color
    ? { backgroundColor: `rgb(${color[0]} ${color[1]} ${color[2]} / 0.15)` }
    : undefined;
  return (
    <tr className="bg-background">
      <td colSpan={COLSPAN} className="px-3 pt-8 pb-3" style={tdStyle}>
        <div className="flex items-center gap-3">
          <span
            aria-hidden
            className="flex h-14 w-14 shrink-0 items-center justify-center rounded-xl bg-muted"
            style={chipStyle}
          >
            {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
            <img src={url} alt="" aria-hidden width={48} height={48} />
          </span>
          <span className="font-semibold text-foreground text-xl">{code}</span>
          <span className="text-muted-fg">·</span>
          <span className="text-muted-fg text-base">{currencyName(code)}</span>
        </div>
      </td>
    </tr>
  );
}

// Returns a function that assigns (code, symbol) to the given side of the swap
// store and navigates to /swap. Delegates the actual mutation to the store's
// atomic `pickSide` action — a single `set` call that handles flip / set-new /
// no-op without ever passing through a transient sameToken state.
//
// `flushSync` wraps the `pickSide` call so React commits every Zustand
// subscriber re-render before `router.push` runs. Without it, the navigation
// transition (a `startTransition` internally) could begin before the store
// update is observed by React's scheduler, leaving the /swap mount to read
// the prior pair in production — particularly when Next.js has prefetched
// the route with the prior store state baked in. flushSync forces the
// commit synchronously, removing that race.
function usePickToken(): (
  side: Side,
  code: IsoCurrencyCode,
  symbol: string,
) => void {
  const router = useRouter();
  const pickSide = useSwapStore((s) => s.pickSide);
  return (side, code, symbol) => {
    flushSync(() => pickSide(side, code, symbol));
    router.push("/swap");
  };
}

function SwapPickerCell({
  code,
  symbol,
}: {
  code: IsoCurrencyCode;
  symbol: string;
}) {
  const pickToken = usePickToken();
  const onFrom = useSwapStore(
    (s) => s.from.currency === code && s.from.stablecoin === symbol,
  );
  const onTo = useSwapStore(
    (s) => s.to.currency === code && s.to.stablecoin === symbol,
  );

  const btn = (side: Side, label: string) => {
    const alreadyOnSide = side === "from" ? onFrom : onTo;
    const onOther = side === "from" ? onTo : onFrom;
    const title = alreadyOnSide
      ? `${symbol} is already your ${label.toLowerCase()} token`
      : onOther
        ? `Flip the swap direction so ${symbol} becomes your ${label.toLowerCase()} token`
        : `Use ${symbol} as your ${label.toLowerCase()} token`;
    return (
      <button
        type="button"
        onClick={() => pickToken(side, code, symbol)}
        disabled={alreadyOnSide}
        title={title}
        className="rounded border border-border bg-background px-2 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
      >
        {label}
      </button>
    );
  };

  return (
    <div className="flex items-center gap-1">
      {btn("from", "From")}
      {btn("to", "To")}
    </div>
  );
}

function StablecoinRow({
  code,
  s,
  rowIndex,
  groupSize,
}: {
  code: IsoCurrencyCode;
  s: Stablecoin;
  rowIndex: number;
  groupSize: number;
}) {
  const striped = groupSize >= 2 && rowIndex % 2 === 1;
  const isLastInGroup = rowIndex === groupSize - 1;
  const info = useTokenInfo(s.mint);
  const change = info?.priceChange24h;
  const changeTone =
    typeof change !== "number"
      ? "text-muted-fg"
      : change > 0
        ? "text-accent-buy"
        : change < 0
          ? "text-accent-sell"
          : "text-muted-fg";
  // Layered with NumberFlow below: rolling digits convey *which* value
  // moved at the digit level, the bg flash adds a brief whole-cell cue
  // that draws the eye on each Jupiter refresh.
  const priceFlash = useFlashOnChange(info?.usdPrice);
  const changeFlash = useFlashOnChange(change);
  const volumeFlash = useFlashOnChange(info?.volume24h);
  const mcapFlash = useFlashOnChange(info?.mcap);
  const liquidityFlash = useFlashOnChange(info?.liquidity);
  const holdersFlash = useFlashOnChange(info?.holderCount);
  return (
    <tr
      id={s.symbol.toLowerCase()}
      className={`scroll-mt-24 border-border border-t ${isLastInGroup ? "border-b" : ""} ${striped ? "bg-muted/70" : ""}`}
    >
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <div className="flex items-center gap-2">
          {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
          <img
            src={s.icon}
            alt=""
            aria-hidden
            width={28}
            height={28}
            className="h-7 w-7 shrink-0 rounded-full"
          />
          <span className="font-mono text-foreground">{s.symbol}</span>
          <CopyButton value={s.symbol} label="token symbol" />
          <Popover.Root>
            <Popover.Trigger
              type="button"
              aria-label={`Show details for ${s.symbol}`}
              className="inline-flex shrink-0 items-center rounded p-0.5 text-muted-fg hover:text-foreground"
            >
              <Info size={12} />
            </Popover.Trigger>
            <Popover.Portal>
              <Popover.Content
                side="top"
                sideOffset={4}
                className="z-50 flex flex-col gap-1.5 rounded-md border border-border bg-background px-3 py-2 text-xs shadow-lg"
              >
                {s.name !== s.symbol && (
                  <div className="font-medium text-foreground">{s.name}</div>
                )}
                {s.issuer.socials?.x && (
                  <div className="flex items-center gap-1">
                    <span className="font-mono text-foreground">
                      @{s.issuer.socials.x}
                    </span>
                    <CopyButton
                      value={`@${s.issuer.socials.x}`}
                      label="X handle"
                    />
                    <a
                      href={xHref(s.issuer.socials.x)}
                      target="_blank"
                      rel="noopener noreferrer"
                      title={`Open @${s.issuer.socials.x} on X`}
                      className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
                    >
                      <ExternalLink size={12} />
                    </a>
                  </div>
                )}
                <div className="flex flex-col gap-0.5 border-border border-t pt-1.5">
                  <div className="text-[10px] text-muted-fg uppercase tracking-wide">
                    {s.issuer.name.length === 1 ? "Issuer:" : "Issuers:"}
                  </div>
                  <ul className="list-disc pl-4 marker:text-muted-fg">
                    {s.issuer.name.map((n) => (
                      <li key={n} className="text-foreground">
                        {n}
                      </li>
                    ))}
                  </ul>
                </div>
              </Popover.Content>
            </Popover.Portal>
          </Popover.Root>
          <a
            href={s.issuer.url}
            target="_blank"
            rel="noopener noreferrer"
            title={`${s.symbol} issuer website`}
            className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
          >
            <ExternalLink size={12} />
          </a>
        </div>
      </td>
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <SwapPickerCell code={code} symbol={s.symbol} />
      </td>
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <div className="flex items-center gap-1">
          <span
            className="whitespace-nowrap font-mono text-foreground text-xs"
            title={s.mint}
          >
            {shortenMint(s.mint)}
          </span>
          <CopyButton value={s.mint} label="mint address" />
          <a
            href={explorerTokenUrl(s.mint)}
            target="_blank"
            rel="noopener noreferrer"
            title={`View ${s.symbol} on Solscan`}
            className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
          >
            <ExternalLink size={12} />
          </a>
          {s.mintSourceUrl && (
            <a
              href={s.mintSourceUrl}
              target="_blank"
              rel="noopener noreferrer"
              title={`Issuer-verified mint source for ${s.symbol}`}
              className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
            >
              <HelpCircle size={12} />
            </a>
          )}
        </div>
      </td>
      <td className="border-border border-r px-3 py-2 text-right align-top font-mono text-foreground tabular-nums last:border-r-0">
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(priceFlash)}`}
        >
          {isFiniteNumber(info?.usdPrice) ? (
            <NumberFlow value={info.usdPrice} format={priceFormat} />
          ) : (
            "—"
          )}
        </span>
      </td>
      <td
        className={`border-border border-r px-3 py-2 text-right align-top font-mono tabular-nums last:border-r-0 ${changeTone}`}
      >
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(changeFlash)}`}
        >
          {isFiniteNumber(change) ? (
            <NumberFlow value={change} format={changeFormat} suffix="%" />
          ) : (
            "—"
          )}
        </span>
      </td>
      <td className="border-border border-r px-3 py-2 text-right align-top font-mono text-foreground tabular-nums last:border-r-0">
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(volumeFlash)}`}
        >
          {isFiniteNumber(info?.volume24h) ? (
            <NumberFlow value={info.volume24h} format={compactUsdFormat} />
          ) : (
            "—"
          )}
        </span>
      </td>
      <td className="border-border border-r px-3 py-2 text-right align-top font-mono text-foreground tabular-nums last:border-r-0">
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(mcapFlash)}`}
        >
          {isFiniteNumber(info?.mcap) ? (
            <NumberFlow value={info.mcap} format={compactUsdFormat} />
          ) : (
            "—"
          )}
        </span>
      </td>
      <td className="border-border border-r px-3 py-2 text-right align-top font-mono text-foreground tabular-nums last:border-r-0">
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(liquidityFlash)}`}
        >
          {isFiniteNumber(info?.liquidity) ? (
            <NumberFlow value={info.liquidity} format={compactUsdFormat} />
          ) : (
            "—"
          )}
        </span>
      </td>
      <td className="border-border border-r px-3 py-2 text-right align-top font-mono text-foreground tabular-nums last:border-r-0">
        <span
          className={`rounded px-1 transition-colors duration-300 ${flashBg(holdersFlash)}`}
        >
          {isFiniteNumber(info?.holderCount) ? (
            <NumberFlow value={info.holderCount} format={compactCountFormat} />
          ) : (
            "—"
          )}
        </span>
      </td>
    </tr>
  );
}

function CurrenciesInner() {
  const searchParams = useSearchParams();
  // `?symbol=<SYM>` (from the picker's `?` link) lands here in strict mode —
  // filter by exact symbol equality so EURC doesn't also surface EURCV. Any
  // user edit to the search input drops strict mode and switches back to the
  // fuzzy `?q=` URL form so the URL stays a copy-paste-able representation
  // of what the user is seeing.
  const initialSymbol = searchParams.get("symbol");
  const [query, setQuery] = useState(
    initialSymbol ?? searchParams.get("q") ?? "",
  );
  const [strict, setStrict] = useState(initialSymbol !== null);
  const [focused, setFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useAppEvent("focusCurrenciesSearch", () => {
    inputRef.current?.focus();
    inputRef.current?.select();
  });

  // Warm the Jupiter token-info cache on mount, then refresh every 10 s so
  // price / 24h Δ / volume / mcap / liquidity / holders stay live while the
  // page is open. Cache writes call notify(), which re-renders every row via
  // `useSyncExternalStore`; <NumberFlow> animates the digits that actually
  // changed.
  useEffect(() => {
    prefetchAllTokenInfo(ALL_STABLECOIN_MINTS);
    const id = window.setInterval(() => {
      prefetchAllTokenInfo(ALL_STABLECOIN_MINTS);
    }, REFRESH_INTERVAL_MS);
    return () => window.clearInterval(id);
  }, []);

  // Mirror the current search into the URL — fuzzy mode writes `q`, strict
  // mode (set only by an initial `?symbol=` from the picker) writes `symbol`.
  // We always clear the other param so refreshes can't end up with both.
  const commitQueryToUrl = (value: string, isStrict: boolean) => {
    const params = new URLSearchParams(window.location.search);
    params.delete(isStrict ? "q" : "symbol");
    const key = isStrict ? "symbol" : "q";
    if (value) params.set(key, value);
    else params.delete(key);
    const search = params.toString();
    const next = `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  };

  const q = query.trim().toLowerCase();
  const lookup = useInfoLookup();
  const [sort, setSort] = useState<SortState>(null);
  const [groupByCurrency, setGroupByCurrency] = useState(true);
  useAppEvent("toggleGroupByCurrency", () => setGroupByCurrency((g) => !g));
  useAppEvent("currenciesSort", (key) => toggleSort(key));
  const toggleSort = (key: SortKey) =>
    setSort((prev) => {
      if (!prev || prev.key !== key) return { key, direction: "desc" };
      if (prev.direction === "desc") return { key, direction: "asc" };
      return null;
    });
  const filtered = useMemo(
    () =>
      SUPPORTED.map((code) => ({
        code,
        stables: CURRENCIES[code].stablecoins.filter((s) =>
          matches(s, code, q, strict),
        ),
      })).filter((g) => g.stables.length > 0),
    [q, strict],
  );
  // Re-sort outside the useMemo: `lookup` reads from the shared cache, which
  // mutates every 10 s on the refresh interval. Keeping sort out of the memo
  // means each cache notify re-renders with freshly ranked stables without
  // having to thread version counters through deps. When no column is
  // actively sorted, fall back to volume-desc within group + JSON order
  // across groups (the default ranking that's been in place since ENG-359).
  const grouped =
    sort === null
      ? filtered.map(({ code, stables }) => ({
          code,
          stables: sortByVolumeDesc(stables, lookup),
        }))
      : filtered
          .map(({ code, stables }) => ({
            code,
            stables: sortStablesByMetric(
              stables,
              sort.key,
              sort.direction,
              lookup,
            ),
            score: groupScore(stables, sort.key, lookup),
          }))
          .sort((a, b) =>
            sort.direction === "desc" ? b.score - a.score : a.score - b.score,
          );

  // Flat (un-grouped) view: pool every filtered stable across currencies and
  // sort by the active column. Default to 24 h volume desc when no header is
  // selected, mirroring the user-visible "Group by currency" off behavior.
  const flatKey: SortKey = sort?.key ?? "volume24h";
  const flatDirection: SortDir = sort?.direction ?? "desc";
  // What the column headers should *show* as active. In flat mode with no
  // explicit sort, surface the implicit "24h Vol desc" so the chevron makes
  // it obvious which column is driving the order.
  const headerSort: SortState =
    sort ?? (!groupByCurrency ? { key: "volume24h", direction: "desc" } : null);
  const flatStables = filtered
    .flatMap(({ code, stables }) => stables.map((s) => ({ code, s })))
    .sort((a, b) => {
      const va = lookup(a.s.mint)?.[flatKey] ?? -1;
      const vb = lookup(b.s.mint)?.[flatKey] ?? -1;
      return flatDirection === "desc" ? vb - va : va - vb;
    });

  const pickToken = usePickToken();
  useAppEvent("pickCurrencyOnlyResult", (side) => {
    if (grouped.length !== 1 || grouped[0].stables.length !== 1) return;
    const { code } = grouped[0];
    const { symbol } = grouped[0].stables[0];
    pickToken(side, code, symbol);
  });

  const stats = currencyStats();

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-56 items-center gap-2 rounded-md border border-border bg-muted px-3">
            <Search size={14} className="shrink-0 text-muted-fg" />
            <input
              ref={inputRef}
              type="text"
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                // Any user edit relaxes strict mode (the picker `?` link
                // boots the page in strict so EURC doesn't match EURCV; once
                // the user touches the box they've signaled they want the
                // ordinary substring search).
                if (strict) setStrict(false);
              }}
              onFocus={() => setFocused(true)}
              onBlur={() => {
                setFocused(false);
                commitQueryToUrl(query, strict);
              }}
              onKeyDown={(e) => {
                if (e.key === "Escape") {
                  e.preventDefault();
                  inputRef.current?.blur();
                } else if (e.key === "Enter") {
                  e.preventDefault();
                  commitQueryToUrl(query, strict);
                  inputRef.current?.blur();
                }
              }}
              placeholder="Search currencies…"
              className="min-w-0 flex-1 bg-transparent text-foreground text-sm outline-none placeholder:text-muted-fg"
            />
            <kbd
              aria-hidden
              title={
                focused ? "Press Esc to exit search" : "Press / to focus search"
              }
              className="hidden shrink-0 rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10px] text-muted-fg sm:inline-block"
            >
              {focused ? "Esc" : "/"}
            </kbd>
            {query && (
              <button
                type="button"
                onClick={() => {
                  setQuery("");
                  if (strict) setStrict(false);
                  commitQueryToUrl("", false);
                }}
                aria-label="Clear search"
                className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-background hover:text-foreground"
              >
                <X size={14} />
              </button>
            )}
          </div>
          <label className="flex select-none items-center gap-2 text-muted-fg text-xs hover:text-foreground">
            <input
              type="checkbox"
              checked={groupByCurrency}
              onChange={(e) => setGroupByCurrency(e.target.checked)}
              className="h-3.5 w-3.5 cursor-pointer accent-accent"
            />
            Group by currency
          </label>
        </div>
        <div className="flex flex-col text-right text-muted-fg text-xs">
          <p>
            <span className="text-foreground">{stats.represented}</span> of{" "}
            <span className="text-foreground">{stats.total}</span> currencies
            represented on Solana
          </p>
          <p>
            <span className="text-foreground">{stats.missing}</span> not yet
            listed
          </p>
        </div>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                Token
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                Swap
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                Mint Address
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0">
                Price
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0">
                24h Δ
              </th>
              <SortableHeader
                sortKey="volume24h"
                label="24h Vol"
                sort={headerSort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="mcap"
                label="Market Cap"
                sort={headerSort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="liquidity"
                label="Liquidity"
                sort={headerSort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="holderCount"
                label="Holders"
                sort={headerSort}
                onToggle={toggleSort}
              />
            </tr>
          </thead>
          <tbody>
            {grouped.length === 0 ? (
              <tr>
                <td
                  colSpan={COLSPAN}
                  className="px-3 py-6 text-center text-muted-fg text-sm"
                >
                  No tokens match
                </td>
              </tr>
            ) : groupByCurrency ? (
              grouped.flatMap(({ code, stables }) => [
                <CurrencyHeaderRow key={`h-${code}`} code={code} />,
                ...stables.map((s, i) => (
                  <StablecoinRow
                    key={s.symbol}
                    code={code}
                    s={s}
                    rowIndex={i}
                    groupSize={stables.length}
                  />
                )),
              ])
            ) : (
              flatStables.map(({ code, s }, i) => (
                <StablecoinRow
                  key={s.symbol}
                  code={code}
                  s={s}
                  rowIndex={i}
                  groupSize={flatStables.length}
                />
              ))
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}

export function CurrenciesView() {
  return (
    <Suspense fallback={null}>
      <CurrenciesInner />
    </Suspense>
  );
}
