"use client";

import NumberFlow from "@number-flow/react";
import { useMemo, useState } from "react";
import {
  ArrowUpDown,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Minus,
  Plus,
} from "@/components/icons";
import {
  VaultActionDialog,
  type VaultActionMode,
} from "@/components/liquidity/VaultActionDialog";
import { CopyButton } from "@/components/ui/CopyButton";
import { shortenMint } from "@/lib/data/currencies";
import {
  marketApr24h,
  marketFees24h,
  marketVolume24h,
  VAULT_MARKETS,
  type Vault,
  type VaultMarket,
  vaultApr24h,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";

const COLSPAN = 5;

type SortKey = "volume24h" | "fees24h" | "apr24h";
type SortDir = "asc" | "desc";
type SortState = { key: SortKey; direction: SortDir } | null;

// Aggregate metric for a market on the active sort column. APR can be null
// (zero TVL); callers below push nulls to the bottom regardless of direction.
const marketMetric = (m: VaultMarket, key: SortKey): number | null => {
  if (key === "volume24h") return marketVolume24h(m);
  if (key === "fees24h") return marketFees24h(m);
  return marketApr24h(m);
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
    <th
      scope="col"
      className="sticky top-14 z-20 border-border border-r bg-muted p-0 last:border-r-0"
    >
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

// Compact USD ("$1.2M") number cell shared by volume + fees columns.
function UsdCell({ value }: { value: number }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono text-foreground tabular-nums last:border-r-0">
      <NumberFlow value={value} format={FORMATS.usdCompact} />
    </td>
  );
}

// APR cell — renders an em dash when null (zero TVL). Toned positive so the
// yield reads as an at-a-glance "good" signal.
function AprCell({ apr }: { apr: number | null }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono tabular-nums text-accent-buy last:border-r-0">
      {apr === null ? (
        <span className="text-muted-fg">—</span>
      ) : (
        <NumberFlow value={apr} format={FORMATS.percent} />
      )}
    </td>
  );
}

function VaultRow({
  vault,
  onAction,
}: {
  vault: Vault;
  onAction: (vault: Vault, mode: VaultActionMode) => void;
}) {
  const depositDisabled = vault.frozen || !vault.outsideDepositsApproved;
  const withdrawDisabled = vault.frozen;
  const actionBtn = (mode: VaultActionMode, disabled: boolean) => {
    const Icon = mode === "deposit" ? Plus : Minus;
    const title = vault.frozen
      ? "This vault is frozen"
      : mode === "deposit" && !vault.outsideDepositsApproved
        ? "Outside deposits not approved for this vault"
        : mode === "deposit"
          ? "Deposit into this vault"
          : "Withdraw from this vault";
    return (
      <button
        type="button"
        onClick={() => onAction(vault, mode)}
        disabled={disabled}
        title={title}
        className="inline-flex items-center gap-1 rounded border border-border bg-background px-2 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
      >
        <Icon size={12} />
        {mode === "deposit" ? "Deposit" : "Withdraw"}
      </button>
    );
  };
  return (
    <tr className="border-border border-t bg-muted/40">
      <td className="border-border border-r py-2 pr-3 pl-12 align-middle last:border-r-0">
        <div className="flex items-center gap-1">
          <span
            className="font-mono text-foreground text-xs"
            title={vault.leader}
          >
            {shortenMint(vault.leader)}
          </span>
          <CopyButton value={vault.leader} label="leader address" />
          <a
            href={explorerAddressUrl(vault.leader)}
            target="_blank"
            rel="noopener noreferrer"
            title="View leader on Solscan"
            className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
          >
            <ExternalLink size={12} />
          </a>
          {vault.frozen && (
            <span className="ml-1 rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
              Frozen
            </span>
          )}
        </div>
      </td>
      <UsdCell value={vault.volume24h} />
      <UsdCell value={vault.fees24h} />
      <AprCell apr={vaultApr24h(vault)} />
      <td className="px-3 py-2 text-right align-middle">
        <div className="flex items-center justify-end gap-1">
          {actionBtn("deposit", depositDisabled)}
          {actionBtn("withdraw", withdrawDisabled)}
        </div>
      </td>
    </tr>
  );
}

function MarketRow({
  market,
  expanded,
  onToggleExpand,
}: {
  market: VaultMarket;
  expanded: boolean;
  onToggleExpand: () => void;
}) {
  const count = market.vaults.length;
  return (
    <tr
      className="cursor-pointer border-border border-t hover:bg-muted/40"
      onClick={onToggleExpand}
    >
      <td className="border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-2">
          <ChevronDown
            size={14}
            className={`shrink-0 text-muted-fg transition-transform ${expanded ? "" : "-rotate-90"}`}
          />
          <span className="flex shrink-0 items-center">
            {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
            <img
              src={market.baseFlagUrl}
              alt=""
              aria-hidden
              width={20}
              height={20}
              className="rounded-sm"
            />
            {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
            <img
              src={market.quoteFlagUrl}
              alt=""
              aria-hidden
              width={20}
              height={20}
              className="-ml-1 rounded-sm ring-1 ring-background"
            />
          </span>
          <span className="font-medium text-foreground">{market.label}</span>
        </div>
      </td>
      <UsdCell value={marketVolume24h(market)} />
      <UsdCell value={marketFees24h(market)} />
      <AprCell apr={marketApr24h(market)} />
      <td className="px-3 py-2 text-right align-middle text-muted-fg text-xs">
        {count} {count === 1 ? "vault" : "vaults"}
      </td>
    </tr>
  );
}

export function LiquidityView() {
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [sort, setSort] = useState<SortState>(null);
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
    mode: VaultActionMode;
  } | null>(null);

  const toggleSort = (key: SortKey) =>
    setSort((prev) => {
      if (!prev || prev.key !== key) return { key, direction: "desc" };
      if (prev.direction === "desc") return { key, direction: "asc" };
      return null;
    });

  const toggleExpand = (key: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const markets = useMemo(() => {
    if (sort === null) return VAULT_MARKETS;
    const { key, direction } = sort;
    return [...VAULT_MARKETS].sort((a, b) => {
      const va = marketMetric(a, key);
      const vb = marketMetric(b, key);
      // Nulls (zero-TVL APR) always sink, whichever way we're sorting.
      if (va === null && vb === null) return 0;
      if (va === null) return 1;
      if (vb === null) return -1;
      return direction === "desc" ? vb - va : va - vb;
    });
  }, [sort]);

  const openDialog = (
    market: VaultMarket,
    vault: Vault,
    mode: VaultActionMode,
  ) => setDialog({ market, vault, mode });

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3">
        <h1 className="font-semibold text-foreground text-lg">Liquidity</h1>
        <p className="text-muted-fg text-sm">
          Back a leader's vault and share in spread capture. Expand a pair to
          see its vaults.
        </p>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[720px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Pair
              </th>
              <SortableHeader
                sortKey="volume24h"
                label="24h Vol"
                sort={sort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="fees24h"
                label="24h Fees"
                sort={sort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="apr24h"
                label="APR 24h"
                sort={sort}
                onToggle={toggleSort}
              />
              <th
                scope="col"
                className="sticky top-14 z-20 bg-muted px-3 py-2 text-right font-medium"
              >
                Vaults
              </th>
            </tr>
          </thead>
          <tbody>
            {markets.length === 0 ? (
              <tr>
                <td
                  colSpan={COLSPAN}
                  className="px-3 py-6 text-center text-muted-fg text-sm"
                >
                  No vaults yet
                </td>
              </tr>
            ) : (
              markets.flatMap((market) => {
                const isOpen = expanded.has(market.marketPubkey);
                return [
                  <MarketRow
                    key={market.marketPubkey}
                    market={market}
                    expanded={isOpen}
                    onToggleExpand={() => toggleExpand(market.marketPubkey)}
                  />,
                  ...(isOpen
                    ? market.vaults.map((vault) => (
                        <VaultRow
                          key={vault.vaultPubkey}
                          vault={vault}
                          onAction={(v, mode) => openDialog(market, v, mode)}
                        />
                      ))
                    : []),
                ];
              })
            )}
          </tbody>
        </table>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          mode={dialog.mode}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
