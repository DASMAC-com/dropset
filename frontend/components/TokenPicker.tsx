"use client";

import * as Popover from "@radix-ui/react-popover";
import { Check, ChevronDown, ExternalLink, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  CURRENCIES,
  currencyFlag,
  currencyName,
  type IsoCurrencyCode,
  type Stablecoin,
  SUPPORTED,
  tokenIconUrl,
} from "@/lib/currencies";
import { type Side, useSwapStore } from "@/lib/store";

const explorerUrl = (mint: string) =>
  `https://explorer.solana.com/address/${mint}`;

export function TokenPicker({ side }: { side: Side }) {
  const currency = useSwapStore((s) => s[side].currency);
  const stablecoin = useSwapStore((s) => s[side].stablecoin);
  const otherSideState = useSwapStore(
    (s) => s[side === "from" ? "to" : "from"],
  );
  const setToken = useSwapStore((s) => s.setToken);
  const setActiveSide = useSwapStore((s) => s.setActiveSide);

  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [copiedSymbol, setCopiedSymbol] = useState<string | null>(null);
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef<(HTMLDivElement | null)[]>([]);

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  const isBlocked = (cur: IsoCurrencyCode, sym: string) =>
    cur === otherSideState.currency && sym === otherSideState.stablecoin;

  const q = query.trim().toLowerCase();
  const matches = (s: Stablecoin, code: IsoCurrencyCode): boolean =>
    !q ||
    s.symbol.toLowerCase().includes(q) ||
    s.name.toLowerCase().includes(q) ||
    s.mint.toLowerCase().includes(q) ||
    code.toLowerCase().includes(q) ||
    CURRENCIES[code].name.toLowerCase().includes(q);

  const grouped = SUPPORTED.map((code) => ({
    code,
    stables: CURRENCIES[code].stablecoins.filter((s) => matches(s, code)),
  })).filter((g) => g.stables.length > 0);

  let runningIdx = 0;
  const indexedGroups = grouped.map(({ code, stables }) => ({
    code,
    stables: stables.map((s) => ({ s, index: runningIdx++ })),
  }));
  const items: { code: IsoCurrencyCode; s: Stablecoin; blocked: boolean }[] =
    [];
  for (const { code, stables } of grouped) {
    for (const s of stables) {
      items.push({ code, s, blocked: isBlocked(code, s.symbol) });
    }
  }

  // biome-ignore lint/correctness/useExhaustiveDependencies: reset highlight when filter or blocked set changes
  useEffect(() => {
    const idx = items.findIndex((it) => !it.blocked);
    setHighlightedIndex(idx === -1 ? 0 : idx);
  }, [query, otherSideState.currency, otherSideState.stablecoin]);

  useEffect(() => {
    itemRefs.current[highlightedIndex]?.scrollIntoView({ block: "nearest" });
  }, [highlightedIndex]);

  const select = (code: IsoCurrencyCode, sym: string) => {
    setToken(side, code, sym);
    setOpen(false);
  };

  const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (items.length === 0) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlightedIndex((i) => {
        for (let step = 1; step <= items.length; step++) {
          const next = (i + step) % items.length;
          if (!items[next].blocked) return next;
        }
        return i;
      });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlightedIndex((i) => {
        for (let step = 1; step <= items.length; step++) {
          const prev = (i - step + items.length) % items.length;
          if (!items[prev].blocked) return prev;
        }
        return i;
      });
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[highlightedIndex];
      if (item && !item.blocked) select(item.code, item.s.symbol);
    }
  };

  const copyMint = async (sym: string, mint: string) => {
    try {
      await navigator.clipboard.writeText(mint);
      setCopiedSymbol(sym);
      setTimeout(
        () => setCopiedSymbol((cur) => (cur === sym ? null : cur)),
        1500,
      );
    } catch {
      // clipboard API unavailable — silently ignore
    }
  };

  const renderStableRow = (
    code: IsoCurrencyCode,
    s: Stablecoin,
    highlighted = false,
  ) => {
    const blocked = isBlocked(code, s.symbol);
    const selected = code === currency && s.symbol === stablecoin;
    const copied = copiedSymbol === s.symbol;
    const active = selected || highlighted;
    return (
      <div
        className={`flex w-full items-center rounded-md text-sm ${
          active ? "bg-muted text-foreground" : "text-muted-fg"
        }`}
      >
        <button
          type="button"
          disabled={blocked}
          onClick={() => select(code, s.symbol)}
          title={blocked ? "Already selected on the other side" : undefined}
          className="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:bg-transparent disabled:hover:text-muted-fg"
        >
          {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
          <img
            src={tokenIconUrl(s.symbol)}
            alt=""
            aria-hidden
            width={20}
            height={20}
            className="h-5 w-5 shrink-0 rounded-full"
          />
          <span className="flex min-w-0 flex-col">
            <span className="font-mono">{s.symbol}</span>
            {s.name !== s.symbol && (
              <span className="truncate text-muted-fg text-xs">{s.name}</span>
            )}
          </span>
        </button>
        <button
          type="button"
          onClick={() => copyMint(s.symbol, s.mint)}
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
              {s.mint.slice(0, 4)}…{s.mint.slice(-4)}
            </>
          )}
        </button>
        <a
          href={explorerUrl(s.mint)}
          target="_blank"
          rel="noopener noreferrer"
          title={`View ${s.symbol} on Solana Explorer`}
          className="mr-1 flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
        >
          <ExternalLink size={12} />
        </a>
      </div>
    );
  };

  const activeStableRaw = CURRENCIES[currency].stablecoins.find(
    (s) => s.symbol === stablecoin,
  );
  const activeStable =
    activeStableRaw && matches(activeStableRaw, currency)
      ? activeStableRaw
      : null;

  return (
    <Popover.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o) setActiveSide(side);
      }}
    >
      <Popover.Trigger className="flex w-fit items-center gap-2 self-start rounded-lg border border-border bg-background px-3 py-2 text-base text-foreground hover:border-accent hover:text-accent">
        {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
        <img
          src={tokenIconUrl(stablecoin)}
          alt=""
          aria-hidden
          width={20}
          height={20}
          className="h-5 w-5 shrink-0 rounded-full"
        />
        <span className="font-mono font-medium">{stablecoin}</span>
        <ChevronDown size={18} />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          align="start"
          sideOffset={6}
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            searchRef.current?.focus();
          }}
          className="z-50 flex max-h-[60vh] w-80 flex-col overflow-hidden rounded-xl border border-border bg-background shadow-lg"
        >
          <div className="flex items-center gap-2 border-border border-b px-3 py-2">
            <Search size={14} className="shrink-0 text-muted-fg" />
            <input
              ref={searchRef}
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Search tokens…"
              className="w-full bg-transparent text-foreground text-sm outline-none placeholder:text-muted-fg"
            />
          </div>
          {activeStable && (
            <div className="border-border border-b p-1">
              {renderStableRow(currency, activeStable)}
            </div>
          )}
          <div className="flex-1 overflow-y-auto p-1">
            {grouped.length === 0 ? (
              <div className="px-3 py-4 text-center text-muted-fg text-sm">
                No tokens match
              </div>
            ) : (
              indexedGroups.map(({ code, stables }) => (
                <div key={code} className="py-1">
                  <div className="mx-2 mb-1 flex items-center gap-1.5 border-border border-b px-0 py-1 text-muted-fg text-xs uppercase tracking-wide">
                    <span aria-hidden className="text-sm leading-none">
                      {currencyFlag(code)}
                    </span>
                    <span className="font-medium">{code}</span>
                    <span className="text-muted-fg">·</span>
                    <span>{currencyName(code)}</span>
                  </div>
                  {stables.map(({ s, index }) => (
                    <div
                      key={`${code}-${s.symbol}`}
                      ref={(el) => {
                        itemRefs.current[index] = el;
                      }}
                    >
                      {renderStableRow(code, s, index === highlightedIndex)}
                    </div>
                  ))}
                </div>
              ))
            )}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
