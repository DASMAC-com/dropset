"use client";

import NumberFlow from "@number-flow/react";
import { useWalletConnection } from "@solana/react-hooks";
import { useCallback, useMemo, useState } from "react";
import { ExternalLink } from "@/components/icons";
import { CopyButton } from "@/components/ui/CopyButton";
import { FlagPair } from "@/components/ui/Flag";
import { SearchBox } from "@/components/ui/SearchBox";
import {
  SortableHeader,
  type SortDir,
  type SortState,
} from "@/components/ui/SortableHeader";
import { VaultActionDialog } from "@/components/vaults/VaultActionDialog";
import { shortenMint } from "@/lib/data/currencies";
import { allTimePnl, positionPnl } from "@/lib/data/pnl";
import {
  MOCK_OWNER,
  userPosition,
  type VaultPosition,
} from "@/lib/data/positions";
import {
  type FxPairGroup,
  type GroupedVault,
  groupMetric,
  type MetricKey,
  VAULT_FX_GROUPS,
  type Vault,
  type VaultMarket,
  vaultApr24h,
  vaultMetric,
  vaultReserveRatio,
} from "@/lib/data/vaults";
import { emit, useAppEvent } from "@/lib/events";
import { explorerAddressUrl } from "@/lib/explorer";
import { FORMATS } from "@/lib/format/formats";
import { groupedRowClassName } from "@/lib/ui/groupedRows";

const APR_TOOLTIP =
  "What you earn in a year from the leader's skill, based on the last 24 hours. This does not count money made or lost when prices move.";

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

// Order two sort values; nulls (zero-TVL APR, etc.) always sink to the bottom
// regardless of direction. Strings (leader, pair) compare case-insensitively.
const cmpMetric = (
  a: number | string | null,
  b: number | string | null,
  direction: SortDir,
): number => {
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  if (typeof a === "string" && typeof b === "string") {
    const c = a.localeCompare(b, undefined, { sensitivity: "base" });
    return direction === "desc" ? -c : c;
  }
  return direction === "desc"
    ? (b as number) - (a as number)
    : (a as number) - (b as number);
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

const pnlTone = (n: number): string =>
  n > 0 ? "text-accent-buy" : n < 0 ? "text-accent-sell" : "text-muted-fg";

// The connected user's position value in a vault, marked at the vault's reserve
// ratio (the display reference price stand-in), with the all-time return %
// below it (same red/green as the dialog's headline). The basket breakdown
// lives in the manage dialog.
function PositionValue({
  vault,
  position,
}: {
  vault: Vault;
  position: VaultPosition;
}) {
  const refNow = vaultReserveRatio(vault) ?? position.entryRefPrice;
  const { currentValue } = positionPnl(position, vault, refNow);
  const at = allTimePnl(position, vault, refNow);
  return (
    <span className="flex flex-col items-end font-mono text-xs tabular-nums">
      <span className="text-foreground">
        <NumberFlow value={currentValue} format={FORMATS.usd} />
      </span>
      <span className={`text-[10px] ${pnlTone(at.allTimePnl)}`}>
        (<NumberFlow value={at.allTimePct} format={FORMATS.signedReturn} />)
      </span>
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
            <span className="text-muted-fg text-base">({group.nickname})</span>
          )}
          <span className="flex items-center gap-3 font-mono text-muted-fg text-xs tabular-nums">
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
          // A held position can be topped off or withdrawn, so the dialog is a
          // general "Manage", not just "Withdraw".
          label: "Manage",
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
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 text-right align-middle last:border-r-0">
        {connected ? (
          position ? (
            <PositionValue vault={vault} position={position} />
          ) : (
            <span className="font-mono text-muted-fg text-xs">$-</span>
          )
        ) : (
          <span className="font-mono text-muted-fg text-xs">—</span>
        )}
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 text-right align-middle last:border-r-0">
        <button
          type="button"
          onClick={action.onClick}
          disabled={action.disabled}
          title={actionTitle}
          className="shrink-0 rounded border border-border bg-background px-3 py-1 font-medium text-foreground text-xs transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:border-border disabled:bg-muted disabled:text-muted-fg disabled:hover:border-border disabled:hover:text-muted-fg"
        >
          {action.label}
        </button>
      </td>
      <AprCell apr={vaultApr24h(vault)} />
      <UsdCell value={vault.tvl} />
      <UsdCell value={vault.volume24h} />
    </tr>
  );
}

export function VaultsView() {
  const { connected } = useWalletConnection();
  const [groupByPair, setGroupByPair] = useState(true);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<MetricKey>>(null);
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
  } | null>(null);

  // A connected wallet is treated as the mock depositor, so the seeded
  // positions surface. Disconnected → no positions. The accessor is the data
  // seam; a real fetch keyed on the connected pubkey drops in here later.
  const owner = connected ? MOCK_OWNER : null;
  const positionFor = useCallback(
    (vaultPubkey: string): VaultPosition | null =>
      owner ? userPosition(owner, vaultPubkey) : null,
    [owner],
  );

  // The connected user's position value in a vault (0 if none) — the sort key
  // "position" can't live in vaultMetric since it depends on wallet state.
  // Memoized so the sort useMemos below have a stable dependency.
  const positionValue = useCallback(
    (vault: Vault): number => {
      const p = positionFor(vault.vaultPubkey);
      return p
        ? positionPnl(p, vault, vaultReserveRatio(vault) ?? 0).currentValue
        : 0;
    },
    [positionFor],
  );
  const vaultSortValue = useCallback(
    (gv: GroupedVault, key: MetricKey): number | string | null => {
      if (key === "position") return positionValue(gv.vault);
      if (key === "leader") return gv.vault.leader;
      if (key === "pair") return gv.market.label;
      return vaultMetric(gv, key);
    },
    [positionValue],
  );
  const groupSortValue = useCallback(
    (g: FxPairGroup, key: MetricKey): number | string | null => {
      if (key === "position")
        return g.vaults.reduce((sum, gv) => sum + positionValue(gv.vault), 0);
      // A pair groups many leaders, so rank it by its alphabetically first.
      if (key === "leader")
        return g.vaults.map((gv) => gv.vault.leader).sort()[0] ?? null;
      if (key === "pair") return g.label;
      return groupMetric(g, key);
    },
    [positionValue],
  );

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
        cmpMetric(groupSortValue(a, key), groupSortValue(b, key), direction),
      )
      .map((group) => ({
        group,
        vaults: group.vaults
          .filter((entry) => matchesQuery(q, group, entry))
          .sort((a, b) =>
            cmpMetric(
              vaultSortValue(a, key),
              vaultSortValue(b, key),
              direction,
            ),
          ),
      }))
      .filter((g) => g.vaults.length > 0);
  }, [effective, q, groupSortValue, vaultSortValue]);

  // Ungrouped: one flat, filtered + sorted list of every vault.
  const flatVaults = useMemo(() => {
    const { key, direction } = effective;
    return ALL_WITH_GROUP.filter(({ group, entry }) =>
      matchesQuery(q, group, entry),
    )
      .map(({ entry }) => entry)
      .sort((a, b) =>
        cmpMetric(vaultSortValue(a, key), vaultSortValue(b, key), direction),
      );
  }, [effective, q, vaultSortValue]);

  const onManage = (market: VaultMarket, vault: Vault) =>
    setDialog({ market, vault });

  // `m` opens the manage dialog for the first vault (in the current display
  // order) where the user holds a position. No-op when disconnected or holding
  // nothing.
  useAppEvent("vaultsManage", () => {
    if (!connected) return;
    const entries = groupByPair ? groups.flatMap((g) => g.vaults) : flatVaults;
    const first = entries.find((e) => positionFor(e.vault.vaultPubkey));
    if (first) setDialog({ market: first.market, vault: first.vault });
  });

  // Columns: Pair, Leader, Your Position, Manage, APR, TVL, 24h Vol.
  const colSpan = 7;
  const hasResults = groupByPair ? groups.length > 0 : flatVaults.length > 0;

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      {/* Center the toolbar + table as one block and size it to the table's
          content, so the toolbar (search left, preview right) lines up with
          the table edges however wide the table ends up. */}
      <div className="mx-auto w-fit max-w-full">
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
          <table className="w-auto text-left text-sm">
            <thead className="text-muted-fg text-xs uppercase">
              <tr>
                <VaultSortHeader
                  sortKey="pair"
                  label="Pair"
                  sort={sort}
                  onToggle={toggleSort}
                  align="left"
                />
                <VaultSortHeader
                  sortKey="leader"
                  label="Leader"
                  sort={sort}
                  onToggle={toggleSort}
                  align="left"
                  thClassName="w-px whitespace-nowrap"
                />
                <VaultSortHeader
                  sortKey="position"
                  label="Your Position"
                  sort={sort}
                  onToggle={toggleSort}
                  thClassName="w-px whitespace-nowrap"
                />
                <th
                  scope="col"
                  className="sticky top-14 z-20 w-px whitespace-nowrap border-border border-r bg-muted px-3 py-2 text-right font-medium"
                >
                  Manage
                </th>
                <VaultSortHeader
                  sortKey="apr24h"
                  label="APR 24h"
                  sort={sort}
                  onToggle={toggleSort}
                  info={APR_TOOLTIP}
                  thClassName="w-px whitespace-nowrap"
                />
                <VaultSortHeader
                  sortKey="tvl"
                  label="TVL"
                  sort={sort}
                  onToggle={toggleSort}
                  thClassName="w-px whitespace-nowrap"
                />
                <VaultSortHeader
                  sortKey="volume24h"
                  label="24h Vol"
                  sort={sort}
                  onToggle={toggleSort}
                  thClassName="w-px whitespace-nowrap"
                />
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
                      position={positionFor(entry.vault.vaultPubkey)}
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
                    position={positionFor(entry.vault.vaultPubkey)}
                    rowIndex={i}
                    groupSize={flatVaults.length}
                    onManage={onManage}
                  />
                ))
              )}
            </tbody>
          </table>
        </div>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={dialog.vault}
          position={positionFor(dialog.vault.vaultPubkey)}
          open={true}
          onOpenChange={(open) => {
            if (!open) setDialog(null);
          }}
        />
      )}
    </div>
  );
}
