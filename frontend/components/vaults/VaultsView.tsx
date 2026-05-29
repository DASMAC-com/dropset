"use client";

import NumberFlow from "@number-flow/react";
import { useWalletConnection } from "@solana/react-hooks";
import { useMemo, useState } from "react";
import {
  ArrowUpDown,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Info,
  Minus,
  Plus,
} from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import {
  VaultActionDialog,
  type VaultActionMode,
} from "@/components/vaults/VaultActionDialog";
import { shortenMint } from "@/lib/data/currencies";
import {
  ALL_VAULTS,
  type FxPairGroup,
  type GroupedVault,
  groupMetric,
  type MetricKey,
  VAULT_FX_GROUPS,
  type Vault,
  type VaultMarket,
  vaultApr24h,
  vaultMetric,
} from "@/lib/data/vaults";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";

const COLSPAN = 7;

const APR_TOOLTIP =
  "APR 24h — the annualized 24h fee yield to depositors, net of the leader's performance share.";

type SortDir = "asc" | "desc";
type SortState = { key: MetricKey; direction: SortDir } | null;

// Order two metric values; nulls (zero-TVL APR) always sink to the bottom
// regardless of direction.
const cmpMetric = (
  a: number | null,
  b: number | null,
  direction: SortDir,
): number => {
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  return direction === "desc" ? b - a : a - b;
};

// Compact USD ("$1.2M") cell. `null` → em dash.
function UsdCell({ value }: { value: number | null }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono text-foreground tabular-nums last:border-r-0">
      {value === null ? (
        <span className="text-muted-fg">—</span>
      ) : (
        <NumberFlow value={value} format={FORMATS.usdCompact} />
      )}
    </td>
  );
}

// APR cell — em dash when null (zero TVL). Toned positive so the yield reads
// as an at-a-glance "good" signal.
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

// The connected user's deposit in a vault. No indexer yet, so it reads an em
// dash when disconnected and $0.00 when connected (no positions in the mock).
function DepositCell({ connected }: { connected: boolean }) {
  return (
    <td className="border-border border-r px-3 py-2 text-right align-middle font-mono text-foreground tabular-nums last:border-r-0">
      {connected ? (
        <NumberFlow value={0} format={FORMATS.usd} />
      ) : (
        <span className="text-muted-fg">—</span>
      )}
    </td>
  );
}

function SortableHeader({
  sortKey,
  label,
  sort,
  onToggle,
  info,
}: {
  sortKey: MetricKey;
  label: string;
  sort: SortState;
  onToggle: (key: MetricKey) => void;
  info?: string;
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
      <div className="flex items-center justify-end gap-1 px-3 py-2">
        <button
          type="button"
          onClick={() => onToggle(sortKey)}
          className={`flex cursor-pointer select-none items-center gap-1 text-right font-medium outline-none transition-colors focus:outline-none focus-visible:outline-none ${active ? "text-foreground" : "text-muted-fg hover:text-foreground"}`}
        >
          {label}
          <Icon size={12} />
        </button>
        {info && (
          <button
            type="button"
            title={info}
            aria-label={info}
            className="inline-flex cursor-help items-center text-muted-fg hover:text-foreground"
          >
            <Info size={11} />
          </button>
        )}
      </div>
    </th>
  );
}

// Two overlapping circular images (flags or token icons).
function PairGlyphs({
  base,
  quote,
  size,
}: {
  base: string;
  quote: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center">
      {/* biome-ignore lint/performance/noImgElement: tiny static asset, no optimization needed */}
      <img
        src={base}
        alt=""
        aria-hidden
        width={size}
        height={size}
        className="rounded-full"
      />
      {/* biome-ignore lint/performance/noImgElement: tiny static asset, no optimization needed */}
      <img
        src={quote}
        alt=""
        aria-hidden
        width={size}
        height={size}
        className="-ml-1.5 rounded-full ring-1 ring-background"
      />
    </span>
  );
}

function LeaderTag({ leader, frozen }: { leader: string; frozen: boolean }) {
  return (
    <span className="flex items-center gap-1">
      <span className="font-mono text-muted-fg text-xs" title={leader}>
        {shortenMint(leader)}
      </span>
      <CopyButton value={leader} label="leader address" />
      <a
        href={explorerAddressUrl(leader)}
        target="_blank"
        rel="noopener noreferrer"
        title="View leader on Solscan"
        className="inline-flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
      >
        <ExternalLink size={12} />
      </a>
      {frozen && (
        <span className="rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
          Frozen
        </span>
      )}
    </span>
  );
}

// FX-pair group header — flags + "EUR / USD" + nickname, plus the summed
// aggregates for the pair. Clickable to expand/collapse its vaults.
function FxGroupRow({
  group,
  expanded,
  connected,
  onToggleExpand,
}: {
  group: FxPairGroup;
  expanded: boolean;
  connected: boolean;
  onToggleExpand: () => void;
}) {
  const count = group.vaults.length;
  return (
    <tr
      className="cursor-pointer border-border border-t bg-background hover:bg-muted/40"
      onClick={onToggleExpand}
    >
      <td className="border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-2">
          <ChevronDown
            size={14}
            className={`shrink-0 text-muted-fg transition-transform ${expanded ? "" : "-rotate-90"}`}
          />
          <PairGlyphs
            base={group.baseFlagUrl}
            quote={group.quoteFlagUrl}
            size={24}
          />
          <span className="font-semibold text-foreground">{group.label}</span>
          {group.nickname && (
            <span className="text-muted-fg text-xs">“{group.nickname}”</span>
          )}
        </div>
      </td>
      <UsdCell value={group.volume24h} />
      <UsdCell value={group.fees24h} />
      <UsdCell value={group.tvl} />
      <AprCell apr={group.apr24h} />
      <DepositCell connected={connected} />
      <td className="px-3 py-2 text-right align-middle text-muted-fg text-xs">
        {count} {count === 1 ? "vault" : "vaults"}
      </td>
    </tr>
  );
}

// One vault row. In grouped mode it's indented under its FX-pair header; in
// ungrouped mode it leads with the pair flags so the FX context is visible.
function VaultRow({
  entry,
  grouped,
  connected,
  onAction,
}: {
  entry: GroupedVault;
  grouped: boolean;
  connected: boolean;
  onAction: (market: VaultMarket, vault: Vault, mode: VaultActionMode) => void;
}) {
  const { market, vault } = entry;
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
        onClick={() => onAction(market, vault, mode)}
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
      <td
        className={`border-border border-r py-2 pr-3 align-middle last:border-r-0 ${grouped ? "pl-10" : "pl-3"}`}
      >
        <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
          {!grouped && (
            <PairGlyphs
              base={market.baseFlagUrl}
              quote={market.quoteFlagUrl}
              size={16}
            />
          )}
          <PairGlyphs
            base={market.baseIconUrl}
            quote={market.quoteIconUrl}
            size={18}
          />
          <span className="font-mono font-medium text-foreground text-xs">
            {market.label}
          </span>
          <span className="text-muted-fg">·</span>
          <LeaderTag leader={vault.leader} frozen={vault.frozen} />
        </div>
      </td>
      <UsdCell value={vault.volume24h} />
      <UsdCell value={vault.fees24h} />
      <UsdCell value={vault.tvl} />
      <AprCell apr={vaultApr24h(vault)} />
      <DepositCell connected={connected} />
      <td className="px-3 py-2 text-right align-middle">
        <div className="flex items-center justify-end gap-1">
          {actionBtn("deposit", depositDisabled)}
          {actionBtn("withdraw", withdrawDisabled)}
        </div>
      </td>
    </tr>
  );
}

export function VaultsView() {
  const { connected } = useWalletConnection();
  const [groupByPair, setGroupByPair] = useState(true);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [sort, setSort] = useState<SortState>(null);
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
    mode: VaultActionMode;
  } | null>(null);

  // There's always an effective sort; default 24h volume desc.
  const effective = sort ?? {
    key: "volume24h" as MetricKey,
    direction: "desc" as SortDir,
  };

  const toggleSort = (key: MetricKey) =>
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

  // Grouped: sort the groups by aggregate, and each group's vaults by the same
  // metric. Recomputed when the sort changes.
  const groups = useMemo(() => {
    const { key, direction } = effective;
    return [...VAULT_FX_GROUPS]
      .sort((a, b) =>
        cmpMetric(groupMetric(a, key), groupMetric(b, key), direction),
      )
      .map((g) => ({
        group: g,
        vaults: [...g.vaults].sort((a, b) =>
          cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
        ),
      }));
  }, [effective]);

  // Ungrouped: one flat, sorted list of every vault.
  const flatVaults = useMemo(() => {
    const { key, direction } = effective;
    return [...ALL_VAULTS].sort((a, b) =>
      cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
    );
  }, [effective]);

  const openDialog = (
    market: VaultMarket,
    vault: Vault,
    mode: VaultActionMode,
  ) => setDialog({ market, vault, mode });

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3 flex items-end justify-between gap-3">
        <div>
          <h1 className="font-semibold text-foreground text-lg">Vaults</h1>
          <p className="text-muted-fg text-sm">
            Back a leader's vault and share in spread capture.
          </p>
        </div>
        <label className="flex select-none items-center gap-2 text-muted-fg text-xs hover:text-foreground">
          <input
            type="checkbox"
            checked={groupByPair}
            onChange={(e) => setGroupByPair(e.target.checked)}
            className="h-3.5 w-3.5 cursor-pointer accent-accent"
          />
          Group by pair
        </label>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[900px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Pair / Vault
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
                sortKey="tvl"
                label="TVL"
                sort={sort}
                onToggle={toggleSort}
              />
              <SortableHeader
                sortKey="apr24h"
                label="APR 24h"
                sort={sort}
                onToggle={toggleSort}
                info={APR_TOOLTIP}
              />
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 text-right font-medium last:border-r-0"
              >
                Your Deposit
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 bg-muted px-3 py-2 text-right font-medium"
              >
                {groupByPair ? "Vaults" : "Actions"}
              </th>
            </tr>
          </thead>
          <tbody>
            {groupByPair
              ? groups.flatMap(({ group, vaults }) => {
                  const isOpen = expanded.has(group.key);
                  return [
                    <FxGroupRow
                      key={group.key}
                      group={group}
                      expanded={isOpen}
                      connected={connected}
                      onToggleExpand={() => toggleExpand(group.key)}
                    />,
                    ...(isOpen
                      ? vaults.map((entry) => (
                          <VaultRow
                            key={entry.vault.vaultPubkey}
                            entry={entry}
                            grouped
                            connected={connected}
                            onAction={openDialog}
                          />
                        ))
                      : []),
                  ];
                })
              : flatVaults.map((entry) => (
                  <VaultRow
                    key={entry.vault.vaultPubkey}
                    entry={entry}
                    grouped={false}
                    connected={connected}
                    onAction={openDialog}
                  />
                ))}
            {!groupByPair && flatVaults.length === 0 && (
              <tr>
                <td
                  colSpan={COLSPAN}
                  className="px-3 py-6 text-center text-muted-fg text-sm"
                >
                  No vaults yet
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          mode={dialog.mode}
          connected={connected}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
