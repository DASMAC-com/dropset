"use client";

import { useRouter, useSearchParams } from "next/navigation";
import { Suspense, useEffect, useMemo, useRef, useState } from "react";
import { CopyButton } from "@/components/CopyButton";
import { ExternalLink, HelpCircle, Search, X } from "@/components/icons";
import {
  CURRENCIES,
  currencyFlagUrl,
  currencyName,
  currencyStats,
  type IsoCurrencyCode,
  type Stablecoin,
  SUPPORTED,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";
import { type Side, useSwapStore } from "@/lib/store";

const COLSPAN = 6;

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

const matches = (s: Stablecoin, code: IsoCurrencyCode, q: string): boolean => {
  if (!q) return true;
  return (
    s.symbol.toLowerCase().includes(q) ||
    s.name.toLowerCase().includes(q) ||
    s.mint.toLowerCase().includes(q) ||
    code.toLowerCase().includes(q) ||
    currencyName(code).toLowerCase().includes(q) ||
    s.issuer.name.some((n) => n.toLowerCase().includes(q))
  );
};

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
// store and navigates to /swap. If the token is already on the opposite side,
// flip the swap direction instead of duplicating; if already on the requested
// side, just navigate. Always marks the clicked side as active so the swap
// page highlights the matching token row on arrival, regardless of which
// branch ran.
function usePickToken(): (
  side: Side,
  code: IsoCurrencyCode,
  symbol: string,
) => void {
  const router = useRouter();
  const setToken = useSwapStore((s) => s.setToken);
  const swapSides = useSwapStore((s) => s.swapSides);
  const setActiveSide = useSwapStore((s) => s.setActiveSide);
  return (side, code, symbol) => {
    const { from, to } = useSwapStore.getState();
    const onFrom = code === from.currency && symbol === from.stablecoin;
    const onTo = code === to.currency && symbol === to.stablecoin;
    const alreadyOnSide = side === "from" ? onFrom : onTo;
    const onOther = side === "from" ? onTo : onFrom;
    if (onOther) swapSides();
    else if (!alreadyOnSide) setToken(side, code, symbol);
    setActiveSide(side);
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
        </div>
      </td>
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <SwapPickerCell code={code} symbol={s.symbol} />
      </td>
      <td className="max-w-[120px] border-border border-r px-3 py-2 align-top last:border-r-0">
        <a
          href={s.issuer.url}
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-start gap-1 text-foreground hover:text-accent"
        >
          <span>{s.name}</span>
          <ExternalLink size={10} className="mt-1.5 shrink-0" />
        </a>
      </td>
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <div className="flex items-start gap-1">
          <span className="whitespace-nowrap font-mono text-foreground text-xs">
            {s.mint}
          </span>
          <CopyButton value={s.mint} label="mint address" />
          <a
            href={explorerAddressUrl(s.mint)}
            target="_blank"
            rel="noopener noreferrer"
            title={`View ${s.symbol} on Solana Explorer`}
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
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        {s.issuer.socials?.x ? (
          <div className="flex items-start gap-1">
            <a
              href={xHref(s.issuer.socials.x)}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-foreground hover:text-accent"
            >
              @{s.issuer.socials.x}
              <ExternalLink size={10} />
            </a>
            <CopyButton value={s.issuer.socials.x} label="X handle" />
          </div>
        ) : (
          ""
        )}
      </td>
      <td className="border-border border-r px-3 py-2 align-top last:border-r-0">
        <div className="flex flex-col">
          {s.issuer.name.map((n, i) => (
            <span
              key={n}
              className={i === 0 ? "text-foreground" : "text-muted-fg"}
            >
              {n}
            </span>
          ))}
        </div>
      </td>
    </tr>
  );
}

function CurrenciesInner() {
  const searchParams = useSearchParams();
  const [query, setQuery] = useState(searchParams.get("q") ?? "");
  const [focused, setFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useAppEvent("focusCurrenciesSearch", () => {
    inputRef.current?.focus();
    inputRef.current?.select();
  });

  const commitQueryToUrl = (value: string) => {
    const params = new URLSearchParams(window.location.search);
    if (value) params.set("q", value);
    else params.delete("q");
    const search = params.toString();
    const next = `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`;
    window.history.replaceState(null, "", next);
  };

  const q = query.trim().toLowerCase();
  const grouped = useMemo(
    () =>
      SUPPORTED.map((code) => ({
        code,
        stables: CURRENCIES[code].stablecoins.filter((s) =>
          matches(s, code, q),
        ),
      })).filter((g) => g.stables.length > 0),
    [q],
  );

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
        <div className="flex h-9 w-56 items-center gap-2 rounded-md border border-border bg-muted px-3">
          <Search size={14} className="shrink-0 text-muted-fg" />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onFocus={() => setFocused(true)}
            onBlur={() => {
              setFocused(false);
              commitQueryToUrl(query);
            }}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                inputRef.current?.blur();
              } else if (e.key === "Enter") {
                e.preventDefault();
                commitQueryToUrl(query);
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
              onClick={() => setQuery("")}
              aria-label="Clear search"
              className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-background hover:text-foreground"
            >
              <X size={14} />
            </button>
          )}
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
                Name
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                Mint Address
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                X handle
              </th>
              <th className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0">
                Issuer(s)
              </th>
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
            ) : (
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
