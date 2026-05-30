"use client";

import NumberFlow from "@number-flow/react";
import { useWalletConnection } from "@solana/react-hooks";
import { useMemo, useState } from "react";
import { ExternalLink } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { SearchBox } from "@/components/ui/SearchBox";
import {
  SortableHeader,
  type SortDir,
  type SortState,
} from "@/components/ui/SortableHeader";
import { VaultActionDialog } from "@/components/vaults/VaultActionDialog";
import { shortenMint } from "@/lib/data/currencies";
import {
  type FxPairGroup,
  type GroupedVault,
  groupMetric,
  type MetricKey,
  positionUsd,
  VAULT_FX_GROUPS,
  type Vault,
  type VaultMarket,
  type VaultPosition,
  vaultApr24h,
  vaultMetric,
} from "@/lib/data/vaults";
import { emit, useAppEvent } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";
import { groupedRowClassName } from "@/lib/ui/groupedRows";

const APR_TOOLTIP =
  "What you earn in a year from trading fees if the last 24 hours kept up. This does not count money made or lost when prices move.";

// Pin the generic shared header to this table's metric keys so the literal
// `sortKey` props type-check against `sort` / `onToggle`.
const VaultSortHeader = SortableHeader<MetricKey>;

// Every vault paired with the FX group it belongs to — used for the ungrouped
// view and for search, which matches against group-level fields (nickname, FX
// label) as well as the per-vault ones.
const ALL_WITH_GROUP: { group: FxPairGroup; entry: GroupedVault }[] =
  VAULT_FX_GROUPS.flatMap((group) =>
    group.vaults.map((entry) => ({ group, entry })),
  );

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

// Substring match across the pair's FX label / nickname / currency names and
// the vault's tokens + leader address.
const matchesQuery = (
  q: string,
  group: FxPairGroup,
  entry: GroupedVault,
): boolean => {
  if (!q) return true;
  return [
    group.label,
    group.nickname,
    group.baseName,
    group.quoteName,
    group.baseCurrency,
    group.quoteCurrency,
    entry.market.base,
    entry.market.quote,
    entry.market.label,
    entry.vault.leader,
  ].some((s) => s.toLowerCase().includes(q));
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

// Trim a token amount to 3 decimals with grouping for the position readout.
const fmtAmount = (n: number): string =>
  Number(n.toFixed(3)).toLocaleString("en-US", { maximumFractionDigits: 3 });

const fmtUsd = (n: number): string =>
  `$${n.toLocaleString("en-US", { maximumFractionDigits: 0 })}`;

// Two flags rendered as full SVGs side by side. The Twemoji artwork is already
// a rounded rectangle, so we don't clip it to a circle (that produced a stray
// border on square flags like CH).
function FlagPair({
  base,
  quote,
  size,
}: {
  base: string;
  quote: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center gap-1">
      {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
      <img src={base} alt="" aria-hidden width={size} height={size} />
      {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
      <img src={quote} alt="" aria-hidden width={size} height={size} />
    </span>
  );
}

// Two overlapping circular token icons (the stablecoin logos are round).
function TokenPair({
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
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
      <img
        src={base}
        alt=""
        aria-hidden
        width={size}
        height={size}
        className="rounded-full"
      />
      {/* biome-ignore lint/performance/noImgElement: small static icon, no optimization needed */}
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

// FX-pair heading spanning the table, mirroring the per-currency headings on
// /currencies: two flags + "EUR / USD" + nickname, with the pair's summed TVL
// / 24h volume / vault count to the right.
function FxGroupHeading({
  group,
  colSpan,
}: {
  group: FxPairGroup;
  colSpan: number;
}) {
  const count = group.vaults.length;
  return (
    <tr className="bg-background">
      <td colSpan={colSpan} className="px-3 pt-8 pb-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <FlagPair
            base={group.baseFlagUrl}
            quote={group.quoteFlagUrl}
            size={48}
          />
          <span className="font-semibold text-foreground text-xl">
            {group.label}
          </span>
          {group.nickname && (
            <span className="text-muted-fg text-base">“{group.nickname}”</span>
          )}
          <span className="ml-auto flex items-center gap-3 font-mono text-muted-fg text-xs tabular-nums">
            <span>
              TVL <NumberFlow value={group.tvl} format={FORMATS.usdCompact} />
            </span>
            <span>
              Vol{" "}
              <NumberFlow value={group.volume24h} format={FORMATS.usdCompact} />
            </span>
            <span>
              {count} {count === 1 ? "vault" : "vaults"}
            </span>
          </span>
        </div>
      </td>
    </tr>
  );
}

// One vault row. The action button is context-aware: connect a wallet first,
// then deposit if you hold no position, or withdraw if you do.
function VaultRow({
  entry,
  grouped,
  connected,
  position,
  rowIndex,
  groupSize,
  onManage,
}: {
  entry: GroupedVault;
  grouped: boolean;
  connected: boolean;
  position: VaultPosition | null;
  rowIndex: number;
  groupSize: number;
  onManage: (market: VaultMarket, vault: Vault) => void;
}) {
  const { market, vault } = entry;

  const action = !connected
    ? {
        label: "Connect",
        disabled: false,
        onClick: () => emit("openWalletModal"),
      }
    : position
      ? {
          label: "Withdraw",
          disabled: false,
          onClick: () => onManage(market, vault),
        }
      : {
          label: "Deposit",
          disabled: vault.frozen || !vault.outsideDepositsApproved,
          onClick: () => onManage(market, vault),
        };
  const actionTitle =
    action.label === "Deposit" && action.disabled
      ? vault.frozen
        ? "This vault is frozen, so deposits are closed"
        : "Outside deposits aren't approved for this vault"
      : undefined;

  return (
    <tr className={groupedRowClassName(rowIndex, groupSize)}>
      <td
        className={`border-border border-r py-2 pr-3 align-middle last:border-r-0 ${grouped ? "pl-10" : "pl-3"}`}
      >
        <div className="flex items-center gap-2">
          {!grouped && (
            <FlagPair
              base={market.baseFlagUrl}
              quote={market.quoteFlagUrl}
              size={16}
            />
          )}
          <TokenPair
            base={market.baseIconUrl}
            quote={market.quoteIconUrl}
            size={28}
          />
          <span className="font-mono font-medium text-foreground">
            {market.label}
          </span>
        </div>
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
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
            <span className="rounded bg-accent-sell/15 px-1.5 py-0.5 font-medium text-[10px] text-accent-sell uppercase tracking-wide">
              Frozen
            </span>
          )}
        </div>
      </td>
      <AprCell apr={vaultApr24h(vault)} />
      <UsdCell value={vault.tvl} />
      <UsdCell value={vault.volume24h} />
      <td className="px-3 py-2 align-middle">
        <div className="flex items-center justify-end gap-3">
          {connected &&
            (position ? (
              <span className="whitespace-nowrap font-mono text-foreground text-xs">
                {fmtAmount(position.base)} {market.base} /{" "}
                {fmtAmount(position.quote)} {market.quote}{" "}
                <span className="text-muted-fg">
                  ({fmtUsd(positionUsd(vault, position))})
                </span>
              </span>
            ) : (
              <span className="font-mono text-muted-fg text-xs">$-</span>
            ))}
          <button
            type="button"
            onClick={action.onClick}
            disabled={action.disabled}
            title={actionTitle}
            className="shrink-0 rounded border border-border bg-background px-3 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
          >
            {action.label}
          </button>
        </div>
      </td>
    </tr>
  );
}

export function VaultsView() {
  const { connected } = useWalletConnection();
  const [groupByPair, setGroupByPair] = useState(true);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<MetricKey>>(null);
  const [positions, setPositions] = useState<Map<string, VaultPosition>>(
    () => new Map(),
  );
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
  } | null>(null);

  // There's always an effective sort; default 24h volume desc.
  const effective: { key: MetricKey; direction: SortDir } = sort ?? {
    key: "volume24h",
    direction: "desc",
  };

  const toggleSort = (key: MetricKey) =>
    setSort((prev) => {
      if (!prev || prev.key !== key) return { key, direction: "desc" };
      if (prev.direction === "desc") return { key, direction: "asc" };
      return null;
    });

  // Keyboard shortcuts (see lib/ui/shortcuts.ts → vaults context).
  useAppEvent("toggleGroupByPair", () => setGroupByPair((g) => !g));
  useAppEvent("vaultsSort", (key) => toggleSort(key));

  const q = query.trim().toLowerCase();

  // Grouped: filter + sort each group's vaults, then sort and keep the groups
  // that still have a match.
  const groups = useMemo(() => {
    const { key, direction } = effective;
    return [...VAULT_FX_GROUPS]
      .sort((a, b) =>
        cmpMetric(groupMetric(a, key), groupMetric(b, key), direction),
      )
      .map((group) => ({
        group,
        vaults: group.vaults
          .filter((entry) => matchesQuery(q, group, entry))
          .sort((a, b) =>
            cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
          ),
      }))
      .filter((g) => g.vaults.length > 0);
  }, [effective, q]);

  // Ungrouped: one flat, filtered + sorted list of every vault.
  const flatVaults = useMemo(() => {
    const { key, direction } = effective;
    return ALL_WITH_GROUP.filter(({ group, entry }) =>
      matchesQuery(q, group, entry),
    )
      .map(({ entry }) => entry)
      .sort((a, b) =>
        cmpMetric(vaultMetric(a, key), vaultMetric(b, key), direction),
      );
  }, [effective, q]);

  const onManage = (market: VaultMarket, vault: Vault) =>
    setDialog({ market, vault });

  const deposit = (vaultPubkey: string, basket: VaultPosition) =>
    setPositions((prev) => new Map(prev).set(vaultPubkey, basket));
  const withdraw = (vaultPubkey: string) =>
    setPositions((prev) => {
      const next = new Map(prev);
      next.delete(vaultPubkey);
      return next;
    });

  // Columns: Pair, Leader, APR, TVL, Vol, Your Position.
  const colSpan = 6;
  const hasResults = groupByPair ? groups.length > 0 : flatVaults.length > 0;

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <SearchBox
            value={query}
            onValueChange={setQuery}
            onClear={() => setQuery("")}
            placeholder="Search pairs…"
            focusEvent="focusVaultsSearch"
          />
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
        <p className="text-muted-fg text-xs">
          <span className="font-medium text-amber-400">Preview.</span> All
          figures shown are mock data.
        </p>
      </div>
      <div className="rounded-lg border border-border">
        <table className="w-full min-w-[860px] text-left text-sm">
          <thead className="text-muted-fg text-xs uppercase">
            <tr>
              <th
                scope="col"
                className="sticky top-14 z-20 border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Pair
              </th>
              <th
                scope="col"
                className="sticky top-14 z-20 w-px whitespace-nowrap border-border border-r bg-muted px-3 py-2 font-medium last:border-r-0"
              >
                Leader
              </th>
              <VaultSortHeader
                sortKey="apr24h"
                label="APR 24h"
                sort={sort}
                onToggle={toggleSort}
                info={APR_TOOLTIP}
              />
              <VaultSortHeader
                sortKey="tvl"
                label="TVL"
                sort={sort}
                onToggle={toggleSort}
              />
              <VaultSortHeader
                sortKey="volume24h"
                label="24h Vol"
                sort={sort}
                onToggle={toggleSort}
              />
              <th
                scope="col"
                className="sticky top-14 z-20 bg-muted px-3 py-2 text-right font-medium"
              >
                Your Position
              </th>
            </tr>
          </thead>
          <tbody>
            {!hasResults ? (
              <tr>
                <td
                  colSpan={colSpan}
                  className="px-3 py-6 text-center text-muted-fg text-sm"
                >
                  No vaults match
                </td>
              </tr>
            ) : groupByPair ? (
              groups.flatMap(({ group, vaults }) => [
                <FxGroupHeading
                  key={`h-${group.key}`}
                  group={group}
                  colSpan={colSpan}
                />,
                ...vaults.map((entry, i) => (
                  <VaultRow
                    key={entry.vault.vaultPubkey}
                    entry={entry}
                    grouped
                    connected={connected}
                    position={positions.get(entry.vault.vaultPubkey) ?? null}
                    rowIndex={i}
                    groupSize={vaults.length}
                    onManage={onManage}
                  />
                )),
              ])
            ) : (
              flatVaults.map((entry, i) => (
                <VaultRow
                  key={entry.vault.vaultPubkey}
                  entry={entry}
                  grouped={false}
                  connected={connected}
                  position={positions.get(entry.vault.vaultPubkey) ?? null}
                  rowIndex={i}
                  groupSize={flatVaults.length}
                  onManage={onManage}
                />
              ))
            )}
          </tbody>
        </table>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          position={positions.get(dialog.vault.vaultPubkey) ?? null}
          onDeposit={(basket) => deposit(dialog.vault.vaultPubkey, basket)}
          onWithdraw={() => withdraw(dialog.vault.vaultPubkey)}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
