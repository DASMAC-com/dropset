"use client";

import { useSearchParams } from "next/navigation";
import { Suspense, useMemo, useState } from "react";
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

const CURRENCY_TINT: Record<string, { chip: string; border: string }> = {
  USD: { chip: "bg-emerald-500/15", border: "border-emerald-500/60" },
  EUR: { chip: "bg-sky-500/15", border: "border-sky-500/60" },
  GBP: { chip: "bg-indigo-500/15", border: "border-indigo-500/60" },
  JPY: { chip: "bg-rose-500/15", border: "border-rose-500/60" },
  AUD: { chip: "bg-amber-500/15", border: "border-amber-500/60" },
  BRL: { chip: "bg-green-500/15", border: "border-green-500/60" },
  CHF: { chip: "bg-red-500/15", border: "border-red-500/60" },
  MXN: { chip: "bg-orange-500/15", border: "border-orange-500/60" },
  NGN: { chip: "bg-teal-500/15", border: "border-teal-500/60" },
  ZAR: { chip: "bg-fuchsia-500/15", border: "border-fuchsia-500/60" },
};
const tintFor = (code: IsoCurrencyCode) =>
  CURRENCY_TINT[code] ?? { chip: "bg-muted", border: "border-border" };

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
  const tint = tintFor(code);
  return (
    <tr className="bg-background">
      <td
        colSpan={COLSPAN}
        className={`border-b ${tint.border} px-3 pt-6 pb-2`}
      >
        <div className="flex items-center gap-2 text-muted-fg text-xs uppercase tracking-wide">
          <span
            aria-hidden
            className={`flex h-8 w-8 shrink-0 items-center justify-center rounded-lg text-2xl leading-none ${tint.chip}`}
          >
            {currencyFlag(code)}
          </span>
          <span className="font-semibold text-foreground text-sm">{code}</span>
          <span className="text-muted-fg">·</span>
          <span>{currencyName(code)}</span>
        </div>
      </td>
    </tr>
  );
}

function StablecoinRow({ s }: { s: Stablecoin }) {
  return (
    <tr
      id={s.symbol.toLowerCase()}
      className="scroll-mt-24 border-border border-t"
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
        represented ·{" "}
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
      <div className="overflow-x-auto rounded-lg border border-border">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="sticky top-14 z-10 bg-muted text-muted-fg text-xs uppercase">
            <tr>
              <th className="px-3 py-2 font-medium">Token</th>
              <th className="px-3 py-2 font-medium">Name</th>
              <th className="px-3 py-2 font-medium">Mint Address</th>
              <th className="px-3 py-2 font-medium">X</th>
              <th className="px-3 py-2 font-medium">Issuer(s)</th>
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
                ...stables.map((s) => <StablecoinRow key={s.symbol} s={s} />),
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
