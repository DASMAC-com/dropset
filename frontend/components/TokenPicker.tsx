"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { useEffect, useRef, useState } from "react";
import {
  CURRENCIES,
  currencyName,
  type IsoCurrencyCode,
  type Stablecoin,
  SUPPORTED,
  tokenIconUrl,
} from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { type Side, useSwapStore, useSwapStoreApi } from "@/lib/store";
import { useSwapNav } from "@/lib/swapUrl";
import { isOnSide as predicateIsOnSide } from "@/lib/tokenSelection";
import { sortByVolumeDesc, useInfoLookup } from "@/lib/useUsdQuote";
import { CurrencyGroupHeader } from "./CurrencyGroupHeader";
import { ChevronDown, Search, X } from "./icons";
import { PickerBalanceCell } from "./PickerBalanceCell";
import { StableTokenIdentity } from "./StableTokenIdentity";

export function TokenPicker({ side }: { side: Side }) {
  const currency = useSwapStore((s) => s[side].currency);
  const stablecoin = useSwapStore((s) => s[side].stablecoin);
  const otherSideState = useSwapStore(
    (s) => s[side === "from" ? "to" : "from"],
  );
  const store = useSwapStoreApi();
  const setActiveSide = useSwapStore((s) => s.setActiveSide);
  const gotoSwap = useSwapNav();

  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [highlightedIndex, setHighlightedIndex] = useState(0);
  const searchRef = useRef<HTMLInputElement>(null);
  const itemRefs = useRef<(HTMLDivElement | null)[]>([]);

  useEffect(() => {
    if (!open) setQuery("");
  }, [open]);

  useAppEvent("openPicker", (which) => {
    if (which !== side) return;
    setOpen(true);
    setActiveSide(side);
  });

  // A picker row is blocked if the token is already on the OTHER side
  // (selecting it would be a direction flip, which this picker doesn't
  // do — callers use the swap-arrow button for that).
  const otherSide: Side = side === "from" ? "to" : "from";
  const isBlocked = (cur: IsoCurrencyCode, sym: string) =>
    predicateIsOnSide(
      {
        fromCurrency: side === "from" ? currency : otherSideState.currency,
        fromStablecoin:
          side === "from" ? stablecoin : otherSideState.stablecoin,
        toCurrency: side === "from" ? otherSideState.currency : currency,
        toStablecoin: side === "from" ? otherSideState.stablecoin : stablecoin,
      },
      otherSide,
      cur,
      sym,
    );

  const q = query.trim().toLowerCase();
  const matches = (s: Stablecoin, code: IsoCurrencyCode): boolean =>
    !q ||
    s.symbol.toLowerCase().includes(q) ||
    s.name.toLowerCase().includes(q) ||
    s.mint.toLowerCase().includes(q) ||
    code.toLowerCase().includes(q) ||
    currencyName(code).toLowerCase().includes(q);

  const lookup = useInfoLookup();
  const grouped = SUPPORTED.map((code) => ({
    code,
    stables: sortByVolumeDesc(
      CURRENCIES[code].stablecoins.filter((s) => matches(s, code)),
      lookup,
    ),
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
    store.getState().setToken(side, code, sym);
    const { from, to } = store.getState();
    gotoSwap(from.stablecoin, to.stablecoin);
    setOpen(false);
  };

  const onSearchKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (items.length === 0) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setHighlightedIndex((i) => {
        for (let step = 1; step <= items.length; step++) {
          const next = (i + step) % items.length;
          const item = items[next];
          if (item && !item.blocked) return next;
        }
        return i;
      });
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlightedIndex((i) => {
        for (let step = 1; step <= items.length; step++) {
          const prev = (i - step + items.length) % items.length;
          const item = items[prev];
          if (item && !item.blocked) return prev;
        }
        return i;
      });
    } else if (e.key === "Enter") {
      e.preventDefault();
      const item = items[highlightedIndex];
      if (item && !item.blocked) select(item.code, item.s.symbol);
    }
  };

  const renderStableRow = (
    code: IsoCurrencyCode,
    s: Stablecoin,
    highlighted = false,
  ) => {
    const blocked = isBlocked(code, s.symbol);
    const selected = code === currency && s.symbol === stablecoin;
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
          <StableTokenIdentity s={s} />
          <PickerBalanceCell
            mint={s.mint}
            decimals={s.decimals}
            symbol={s.symbol}
          />
        </button>
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
    <Dialog.Root
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        if (o) setActiveSide(side);
      }}
    >
      <Dialog.Trigger
        className={`flex w-fit items-center gap-2 self-center rounded-lg border border-border bg-background px-4 py-2.5 text-xl text-foreground outline-none ${
          side === "to"
            ? "hover:border-accent-buy hover:text-accent-buy focus-visible:border-accent-buy focus-visible:text-accent-buy"
            : "hover:border-accent hover:text-accent focus-visible:border-accent focus-visible:text-accent"
        }`}
      >
        {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
        <img
          src={tokenIconUrl(stablecoin)}
          alt=""
          aria-hidden
          width={24}
          height={24}
          className="h-6 w-6 shrink-0 rounded-full"
        />
        <span className="font-mono font-medium">{stablecoin}</span>
        <ChevronDown size={20} />
      </Dialog.Trigger>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-[60] bg-background/80 backdrop-blur-2xl" />
        <Dialog.Content
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            searchRef.current?.focus();
          }}
          aria-describedby={undefined}
          className="-translate-x-1/2 fixed top-6 left-1/2 z-[70] flex max-h-[calc(100vh-3rem)] w-[min(420px,calc(100vw-2rem))] flex-col overflow-hidden rounded-xl border border-border bg-background shadow-lg"
        >
          <Dialog.Title className="sr-only">
            Select {side === "from" ? "From" : "To"} token
          </Dialog.Title>
          <div className="flex items-center gap-2 border-border border-b px-3 py-2">
            <Search size={14} className="shrink-0 text-muted-fg" />
            <input
              ref={searchRef}
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Search tokens…"
              aria-label="Search tokens by symbol, name, or mint"
              className="min-w-0 flex-1 bg-transparent text-foreground text-sm outline-none placeholder:text-muted-fg"
            />
            <kbd className="hidden shrink-0 rounded border border-border bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-fg sm:inline-block">
              Esc
            </kbd>
            <Dialog.Close
              aria-label="Close"
              className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-muted hover:text-foreground"
            >
              <X size={14} />
            </Dialog.Close>
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
                  <CurrencyGroupHeader code={code} />
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
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
