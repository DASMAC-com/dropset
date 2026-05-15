"use client";

import { useSearchParams } from "next/navigation";
import { Suspense, useEffect, useMemo, useState } from "react";
import { CopyButton } from "@/components/CopyButton";
import { ExternalLink, HelpCircle, Search, X } from "@/components/icons";
import {
  CURRENCIES,
  currencyFlag,
  currencyName,
  currencyStats,
  type IsoCurrencyCode,
  type Stablecoin,
  SUPPORTED,
} from "@/lib/currencies";
import { explorerAddressUrl } from "@/lib/explorer";

const COLSPAN = 5;

// Cache of dominant color (RGB triplet) computed from a flag emoji rendered to
// a canvas. Module-level so it persists across re-renders / search filters.
type Rgb = [number, number, number];
const flagColorCache = new Map<string, Rgb | null>();

const computeFlagColor = (emoji: string): Rgb | null => {
  if (typeof document === "undefined") return null;
  const size = 24;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d", { willReadFrequently: true });
  if (!ctx) return null;
  ctx.clearRect(0, 0, size, size);
  ctx.font = `${size}px "Apple Color Emoji","Segoe UI Emoji","Noto Color Emoji",sans-serif`;
  ctx.textBaseline = "top";
  ctx.fillText(emoji, 0, 0);
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

const useFlagColor = (code: IsoCurrencyCode, emoji: string): Rgb | null => {
  const [color, setColor] = useState<Rgb | null>(() =>
    flagColorCache.has(code) ? (flagColorCache.get(code) ?? null) : null,
  );
  useEffect(() => {
    if (flagColorCache.has(code)) return;
    const c = computeFlagColor(emoji);
    flagColorCache.set(code, c);
    setColor(c);
  }, [code, emoji]);
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
  const flag = currencyFlag(code);
  const color = useFlagColor(code, flag);
  const borderStyle = color
    ? { borderBottomColor: `rgb(${color[0]} ${color[1]} ${color[2]} / 0.6)` }
    : undefined;
  const chipStyle = color
    ? { backgroundColor: `rgb(${color[0]} ${color[1]} ${color[2]} / 0.15)` }
    : undefined;
  return (
    <tr className="bg-background">
      <td
        colSpan={COLSPAN}
        className="border-border border-b-2 px-3 pt-8 pb-3"
        style={borderStyle}
      >
        <div className="flex items-center gap-3">
          <span
            aria-hidden
            className="flex h-12 w-12 shrink-0 items-center justify-center rounded-xl bg-muted text-4xl leading-none"
            style={chipStyle}
          >
            {flag}
          </span>
          <span className="font-semibold text-foreground text-xl">{code}</span>
          <span className="text-muted-fg">·</span>
          <span className="text-muted-fg text-base">{currencyName(code)}</span>
        </div>
      </td>
    </tr>
  );
}

function StablecoinRow({
  s,
  rowIndex,
  groupSize,
}: {
  s: Stablecoin;
  rowIndex: number;
  groupSize: number;
}) {
  const striped = groupSize >= 2 && rowIndex % 2 === 1;
  return (
    <tr
      id={s.symbol.toLowerCase()}
      className={`scroll-mt-24 border-border border-t ${striped ? "bg-muted/30" : ""}`}
    >
      <td className="px-3 py-2 align-top">
        <div className="flex items-center gap-2">
          {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
          <img
            src={s.icon}
            alt=""
            aria-hidden
            width={20}
            height={20}
            className="h-5 w-5 shrink-0 rounded-full"
          />
          <span className="font-mono text-foreground">{s.symbol}</span>
          <CopyButton value={s.symbol} label="token symbol" />
        </div>
      </td>
      <td className="max-w-[120px] px-3 py-2 align-top">
        <a
          href={s.issuer.url}
          target="_blank"
          rel="noopener noreferrer"
          className="text-muted-fg hover:text-accent"
        >
          {s.name}
          <ExternalLink
            size={10}
            className="ml-1 inline-block shrink-0 align-text-bottom"
          />
        </a>
      </td>
      <td className="px-3 py-2 align-top">
        <div className="flex items-start gap-1">
          <span className="break-all font-mono text-muted-fg text-xs">
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
      <td className="px-3 py-2 align-top">
        {s.issuer.socials?.x ? (
          <div className="flex items-start gap-1">
            <a
              href={xHref(s.issuer.socials.x)}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-1 text-muted-fg hover:text-accent"
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
      <td className="px-3 py-2 align-top">
        <div className="flex flex-col">
          {s.issuer.name.map((n, i) => (
            <span
              key={n}
              className={i % 2 === 0 ? "text-foreground" : "text-muted-fg"}
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
  const initialQuery = searchParams.get("q") ?? "";
  const [query, setQuery] = useState(initialQuery);

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

  const stats = currencyStats();

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <p className="mb-2 text-muted-fg text-xs">
        <span className="text-foreground">{stats.represented}</span> of{" "}
        <span className="text-foreground">{stats.total}</span> currencies
        represented on Solana ·{" "}
        <span className="text-foreground">{stats.missing}</span> not yet listed
      </p>
      <div className="mb-2 flex items-center gap-2 rounded-lg border border-border bg-muted px-3 py-2">
        <Search size={14} className="shrink-0 text-muted-fg" />
        <input
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search by symbol, name, issuer, currency, or mint…"
          className="min-w-0 flex-1 bg-transparent text-foreground text-sm outline-none placeholder:text-muted-fg"
        />
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
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th className="sticky top-14 z-20 bg-muted px-3 py-2 font-medium">
                Token
              </th>
              <th className="sticky top-14 z-20 bg-muted px-3 py-2 font-medium">
                Name
              </th>
              <th className="sticky top-14 z-20 bg-muted px-3 py-2 font-medium">
                Mint Address
              </th>
              <th className="sticky top-14 z-20 bg-muted px-3 py-2 font-medium">
                X
              </th>
              <th className="sticky top-14 z-20 bg-muted px-3 py-2 font-medium">
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
