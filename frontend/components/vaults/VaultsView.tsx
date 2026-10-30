"use client";

import NumberFlow from "@number-flow/react";
import { useWalletConnection } from "@solana/react-hooks";
import { useSearchParams } from "next/navigation";
import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { Crosshair, RefreshCw, X } from "@/components/icons";
import { FlagPair } from "@/components/ui/Flag";
import { SearchBox } from "@/components/ui/SearchBox";
import {
  compareSortValues,
  SortableHeader,
  type SortDir,
  type SortState,
} from "@/components/ui/SortableHeader";
import { TokenIcon } from "@/components/ui/TokenIcon";
import { VaultActionDialog } from "@/components/vaults/VaultActionDialog";
import { shortenMint } from "@/lib/data/currencies";
import { allTimePnl, positionPnl } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  contributeClosedReason,
  type FxPairGroup,
  findVaultBySeq,
  type GroupedVault,
  groupMetric,
  groupVaultMarkets,
  isVaultPinned,
  type MetricKey,
  resolveLegacyLeaderPin,
  type Vault,
  type VaultMarket,
  type VaultPin,
  vaultLabel,
  vaultMetric,
  vaultNumberLabel,
  vaultPinMatches,
  vaultRefPrice,
} from "@/lib/data/vaults";
import { realizedVault } from "@/lib/eclob/vaultTx";
import { emit, useAppEvent } from "@/lib/events";
import { FORMATS } from "@/lib/format/formats";
import { useChainVaults } from "@/lib/hooks/useChainVaults";
import { groupedRowClassName } from "@/lib/ui/groupedRows";
import { pnlTone } from "@/lib/ui/pnlTone";
import {
  replaceUrlParams,
  useGoToSwapPair,
  VAULT_SEQ_PARAM,
  vaultPinFromParams,
  vaultPinParams,
} from "@/lib/ui/swapUrl";

// The table shows only what the chain gives. APR, 24h volume and the pair's
// 24h figures need trade history, so they return with the indexer rather than
// rendering blank here; a depositor's PnL stays on their position, marked at
// the vault's reference price (vaultRefPrice).

// Pin the generic shared header to this table's metric keys so the literal
// `sortKey` props type-check against `sort` / `onToggle`.
const VaultSortHeader = SortableHeader<MetricKey>;

// Shared null-sinking, case-insensitive comparator (see SortableHeader).
const cmpMetric = compareSortValues;

// A `leader` pin longer than this is treated as a full pubkey and shortened in
// its chip; a shorter value is a hand-typed prefix and shown verbatim.
const LEADER_SLUG_MAX = 12;

// Substring match across the pair's FX label / currency names and
// the vault's tokens, name, vault number and leader address.
const matchesQuery = (
  q: string,
  group: FxPairGroup,
  entry: GroupedVault,
): boolean => {
  if (!q) return true;
  return [
    group.label,
    group.baseName,
    group.quoteName,
    group.baseCurrency,
    group.quoteCurrency,
    entry.market.base,
    entry.market.quote,
    entry.market.label,
    entry.vault.name ?? "",
    vaultNumberLabel(entry.vault),
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

// The connected user's position value in a vault, marked at the vault's
// reference price (vaultRefPrice), with the all-time return %
// below it (same red/green as the dialog's headline). The basket breakdown
// lives in the manage dialog.
function PositionValue({
  vault,
  position,
}: {
  vault: Vault;
  position: VaultPosition;
}) {
  const refNow = vaultRefPrice(vault) ?? position.entryRefPrice;
  // Against the fee-realized share count, so it matches what a withdraw pays.
  const realized = realizedVault(vault);
  const { currentValue } = positionPnl(position, realized, refNow);
  const at = allTimePnl(position, realized, refNow);
  return (
    <span className="flex flex-col items-end font-mono text-xs tabular-nums">
      <span className="text-foreground">
        <NumberFlow value={currentValue} format={FORMATS.usd} />
      </span>
      <span
        className={`text-[10px] ${pnlTone(at.allTimePnl, "text-muted-fg")}`}
      >
        (<NumberFlow value={at.allTimePct} format={FORMATS.signedReturn} />)
      </span>
    </span>
  );
}

// Two overlapping circular token icons (the stablecoin logos are round).
// Named `*Symbol` because the sibling FlagPair on the same row takes URLs, and
// a bare `base`/`quote` pair would make the two indistinguishable at the call
// site; TokenIcon resolves each symbol's mirror and remote fallback itself.
function TokenPair({
  baseSymbol,
  quoteSymbol,
  size,
}: {
  baseSymbol: string;
  quoteSymbol: string;
  size: number;
}) {
  return (
    <span className="flex shrink-0 items-center">
      <TokenIcon symbol={baseSymbol} size={size} className="rounded-full" />
      <TokenIcon
        symbol={quoteSymbol}
        size={size}
        className="-ml-1.5 rounded-full ring-1 ring-background"
      />
    </span>
  );
}

// FX-pair heading spanning the table, mirroring the per-currency headings on
// /currencies: two flags + "EUR / USD", with the pair's summed TVL
// and vault count to the right.
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
          <span className="flex items-center gap-3 font-mono text-muted-fg text-xs tabular-nums">
            <span>
              TVL <NumberFlow value={group.tvl} format={FORMATS.usdCompact} />
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
  pinned,
  rowIndex,
  groupSize,
  onManage,
  onPin,
}: {
  entry: GroupedVault;
  grouped: boolean;
  connected: boolean;
  position: VaultPosition | null;
  // Whether the URL filter is pinned to exactly this vault.
  pinned: boolean;
  rowIndex: number;
  groupSize: number;
  onManage: (market: VaultMarket, vault: Vault) => void;
  onPin: (entry: GroupedVault) => void;
}) {
  const { market, vault } = entry;
  const goToSwapPair = useGoToSwapPair();
  const closedReason = contributeClosedReason(market, vault);

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
          disabled: closedReason !== null,
          onClick: () => onManage(market, vault),
        };
  const actionTitle =
    action.label === "Deposit" && action.disabled
      ? (closedReason ?? undefined)
      : undefined;

  return (
    <tr className={groupedRowClassName(rowIndex, groupSize)}>
      <td className="whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-2">
          {!grouped && (
            <FlagPair
              base={market.baseFlagUrl}
              quote={market.quoteFlagUrl}
              size={16}
            />
          )}
          <TokenPair
            baseSymbol={market.base}
            quoteSymbol={market.quote}
            size={28}
          />
          <span className="font-mono font-medium text-foreground">
            {market.label}
          </span>
          {/* Jump to /swap with this pair loaded into the store. */}
          <button
            type="button"
            onClick={() =>
              goToSwapPair(
                { currency: market.baseCurrency, stablecoin: market.base },
                { currency: market.quoteCurrency, stablecoin: market.quote },
              )
            }
            title={`Swap ${market.label}`}
            aria-label={`Swap ${market.label}`}
            className="ml-1 inline-flex shrink-0 items-center rounded p-1 text-muted-fg transition-colors hover:bg-muted hover:text-accent"
          >
            <RefreshCw size={14} />
          </button>
        </div>
      </td>
      <td className="w-px whitespace-nowrap border-border border-r px-3 py-2 align-middle last:border-r-0">
        <div className="flex items-center gap-1">
          {/* The vault number is the identity (leadership rotates), so it
              leads, ahead of the leader-set name, which can't then pass for
              another vault's number. The leader is an attribute: the hover
              keeps the full address; its copy and explorer links live in
              the dialog. */}
          <span
            className="shrink-0 font-mono text-foreground text-xs"
            title={`${vaultLabel(vault)} — Leader ${vault.leader}`}
          >
            {vaultNumberLabel(vault)}
          </span>
          {vault.name && (
            <span className="max-w-48 truncate font-medium text-foreground text-sm">
              {vault.name}
            </span>
          )}
          {/* Pin the view to this exact vault — writes base/quote/vault into
              the URL (a shareable deep link); click again to clear. */}
          <button
            type="button"
            onClick={() => onPin(entry)}
            aria-label={pinned ? "Clear vault filter" : "Pin this vault"}
            title={
              pinned
                ? "Clear vault filter"
                : "Pin this vault — filters the table and updates the shareable URL"
            }
            className={`inline-flex shrink-0 items-center rounded p-1 transition-colors hover:bg-muted ${
              pinned ? "text-accent" : "text-muted-fg hover:text-accent"
            }`}
          >
            <Crosshair size={12} />
          </button>
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
      <UsdCell value={vault.tvl} />
    </tr>
  );
}

// A removable filter chip for an active URL pin.
function FilterChip({
  label,
  onClear,
}: {
  label: string;
  onClear: () => void;
}) {
  return (
    <span className="inline-flex items-center gap-1 rounded-full border border-border bg-muted py-1 pr-1 pl-2.5 font-mono text-foreground text-xs">
      {label}
      <button
        type="button"
        onClick={onClear}
        aria-label={`Clear ${label} filter`}
        className="inline-flex items-center rounded-full p-0.5 text-muted-fg transition-colors hover:bg-background hover:text-foreground"
      >
        <X size={12} />
      </button>
    </span>
  );
}

// A full-width message row in the table body.
function MessageRow({ colSpan, text }: { colSpan: number; text: string }) {
  return (
    <tr>
      <td
        colSpan={colSpan}
        className="px-3 py-6 text-center text-muted-fg text-sm"
      >
        {text}
      </td>
    </tr>
  );
}

// The table's rows, filtered and sorted both ways.
type ArrangedSection = {
  groups: { group: FxPairGroup; vaults: GroupedVault[] }[];
  flat: GroupedVault[];
};

// Filter + sort the groups: grouped (each group's vaults filtered and sorted,
// then the groups sorted, dropping any left empty) and flat (every vault,
// filtered and sorted as one list).
function arrangeSection(
  sectionGroups: FxPairGroup[],
  keep: (group: FxPairGroup, entry: GroupedVault) => boolean,
  cmpGroup: (a: FxPairGroup, b: FxPairGroup) => number,
  cmpVault: (a: GroupedVault, b: GroupedVault) => number,
): ArrangedSection {
  const groups = [...sectionGroups]
    .sort(cmpGroup)
    .map((group) => ({
      group,
      vaults: group.vaults.filter((entry) => keep(group, entry)).sort(cmpVault),
    }))
    .filter((g) => g.vaults.length > 0);
  const flat = sectionGroups
    .flatMap((group) => group.vaults.filter((entry) => keep(group, entry)))
    .sort(cmpVault);
  return { groups, flat };
}

function VaultsInner() {
  const { connected } = useWalletConnection();
  const chain = useChainVaults();
  const liveGroups = useMemo(
    () => groupVaultMarkets(chain.markets),
    [chain.markets],
  );
  // Whether any live vault's TVL fell back to indicative token prices (a dark
  // book or a pair with no USD leg), so the page note says only what's true.
  const liveIndicative = chain.markets.some((m) =>
    m.vaults.some((v) => v.usdIndicative),
  );
  const searchParams = useSearchParams();
  const [groupByPair, setGroupByPair] = useState(true);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState<MetricKey>>(null);
  const [dialog, setDialog] = useState<{
    market: VaultMarket;
    vault: Vault;
  } | null>(null);

  // Seed the pin from the URL once, then own it in state. Updates write back to
  // the URL via replaceState (no router transition) so the address bar always
  // reflects — and can reproduce — the current filter, mirroring /currencies.
  // The pin (VaultPin) is driven by the `?base=&quote=&vault=` URL params, so
  // a filtered view is a shareable deep link to a vault that survives a
  // leader rotation.
  const [pin, setPin] = useState<VaultPin>(() =>
    vaultPinFromParams(searchParams),
  );
  // Merge against the current pin and apply the URL write as a plain side
  // effect — NOT inside the setPin updater, which React may re-run during
  // render (replaceState there pokes the Router mid-render). updatePin runs
  // from event handlers and the one-shot legacy resolve below, so the `pin`
  // closure is current.
  const updatePin = (next: Partial<VaultPin>) => {
    const merged = { ...pin, ...next };
    setPin(merged);
    replaceUrlParams(vaultPinParams(merged));
  };

  // An old leader-keyed link resolves, on the first live read, to the vault
  // number it named, which then replaces the leader in the pin and the URL.
  // One that names no single vault keeps filtering by leader prefix, as it
  // did. Tried once. One-release fallback; drop it with LEGACY_LEADER_PARAM.
  const [legacyPending, setLegacyPending] = useState(
    () => pin.leader !== "" && pin.vault === "",
  );
  useEffect(() => {
    if (!legacyPending || chain.status !== "ready") return;
    setLegacyPending(false);
    const seq = resolveLegacyLeaderPin(
      chain.markets,
      pin.base,
      pin.quote,
      pin.leader,
    );
    if (seq !== null) updatePin({ vault: seq.toString(), leader: "" });
  });

  // A public vault link (vaultLinkHref) names one live vault by its pair and
  // `seq`. Captured once from the URL; when the first live read lands, that
  // vault's dialog opens if it lists the vault, and the link is spent either
  // way.
  const [link, setLink] = useState(() => {
    const seq = searchParams.get(VAULT_SEQ_PARAM);
    return seq === null
      ? null
      : {
          base: searchParams.get("base") ?? "",
          quote: searchParams.get("quote") ?? "",
          seq,
        };
  });
  useEffect(() => {
    // An errored first read is retried by the next poll, so the link waits
    // for a read that lands rather than being spent on the failure.
    if (link === null || chain.status !== "ready") return;
    const hit = findVaultBySeq(chain.markets, link.base, link.quote, link.seq);
    if (hit) setDialog(hit);
    // A link that names no vault leaves the pair pin but not the dead `seq`.
    else replaceUrlParams({ [VAULT_SEQ_PARAM]: null });
    setLink(null);
  }, [link, chain.status, chain.markets]);

  // Re-sync the pin from the URL on browser Back/Forward. Our own writes use
  // replaceState (no navigation), so only popstate can move the URL out from
  // under the state — without this the table/chips would keep a stale pin.
  useEffect(() => {
    const onPop = () =>
      setPin(vaultPinFromParams(new URLSearchParams(window.location.search)));
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

  const matchesPin = useCallback(
    (entry: GroupedVault): boolean =>
      vaultPinMatches(pin, entry.market, entry.vault),
    [pin],
  );

  // A vault's position is the connected wallet's VaultDepositor account.
  const positionFor = useCallback(
    (vault: Vault): VaultPosition | null =>
      chain.positions.get(vault.vaultPubkey) ?? null,
    [chain.positions],
  );

  // The dialog's vault as the latest poll has it. The row is captured at
  // click, but a vault's reserves, shares and gates move while the dialog
  // stays open (its own deposit or withdraw moves them), and the previews and
  // the deposit gate must read the current state. A vault the latest poll no
  // longer lists keeps the captured row.
  const currentVault = (captured: Vault): Vault =>
    chain.markets
      .flatMap((m) => m.vaults)
      .find((v) => v.vaultPubkey === captured.vaultPubkey) ?? captured;

  // The connected user's position value in a vault (0 if none) — the sort key
  // "position" can't live in vaultMetric since it depends on wallet state.
  // Memoized so the sort useMemos below have a stable dependency.
  const positionValue = useCallback(
    (vault: Vault): number => {
      const p = positionFor(vault);
      return p
        ? positionPnl(
            p,
            realizedVault(vault),
            vaultRefPrice(vault) ?? p.entryRefPrice,
          ).currentValue
        : 0;
    },
    [positionFor],
  );
  const vaultSortValue = useCallback(
    (gv: GroupedVault, key: MetricKey): number | string | null => {
      if (key === "position") return positionValue(gv.vault);
      if (key === "name") return vaultLabel(gv.vault);
      if (key === "pair") return gv.market.label;
      return vaultMetric(gv, key);
    },
    [positionValue],
  );
  const groupSortValue = useCallback(
    (g: FxPairGroup, key: MetricKey): number | string | null => {
      if (key === "position")
        return g.vaults.reduce((sum, gv) => sum + positionValue(gv.vault), 0);
      // A pair groups many vaults, so rank it by its first label in the
      // row-level name order (cmpMetric ascending).
      if (key === "name")
        return (
          g.vaults
            .map((gv) => vaultLabel(gv.vault))
            .sort((a, b) => cmpMetric(a, b, "asc"))[0] ?? null
        );
      if (key === "pair") return g.label;
      return groupMetric(g, key);
    },
    [positionValue],
  );

  // There's always an effective sort; default TVL desc. Memoized so the
  // groups/flatVaults memos below don't recompute every render in the default
  // (unsorted) state from a fresh object identity.
  const effective: { key: MetricKey; direction: SortDir } = useMemo(
    () => sort ?? { key: "tvl", direction: "desc" },
    [sort],
  );

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

  const live = useMemo(() => {
    const { key, direction } = effective;
    const keep = (group: FxPairGroup, entry: GroupedVault) =>
      matchesQuery(q, group, entry) && matchesPin(entry);
    const cmpGroup = (a: FxPairGroup, b: FxPairGroup) =>
      cmpMetric(groupSortValue(a, key), groupSortValue(b, key), direction);
    const cmpVault = (a: GroupedVault, b: GroupedVault) =>
      cmpMetric(vaultSortValue(a, key), vaultSortValue(b, key), direction);
    return arrangeSection(liveGroups, keep, cmpGroup, cmpVault);
  }, [liveGroups, effective, q, matchesPin, groupSortValue, vaultSortValue]);

  const onManage = (market: VaultMarket, vault: Vault) =>
    setDialog({ market, vault });

  // The pin "crosshair" on each row toggles the URL filter to that exact vault.
  const isPinnedVault = (e: GroupedVault) =>
    isVaultPinned(pin, e.market, e.vault);
  const togglePinVault = (e: GroupedVault) =>
    updatePin(
      isPinnedVault(e)
        ? { base: "", quote: "", vault: "", leader: "" }
        : {
            base: e.market.base,
            quote: e.market.quote,
            vault: e.vault.chain.seq.toString(),
            leader: "",
          },
    );

  // `m` opens the manage dialog when the current filters resolve to exactly one
  // vault, like /currencies' f/t lone-result picks. The flat list holds the
  // same rows as the groups, so it serves either view. The dialog itself
  // shows Connect / Deposit / Manage as appropriate.
  useAppEvent("vaultsManageOnlyResult", () => {
    const only = live.flat.length === 1 ? live.flat[0] : null;
    if (only) setDialog({ market: only.market, vault: only.vault });
  });

  // Columns: Pair, Name, Your Position, Manage, TVL.
  const colSpan = 5;

  // The rows: FX headings over their vaults when grouped, else the flat list.
  const sectionRows = (section: ArrangedSection) =>
    groupByPair
      ? section.groups.flatMap(({ group, vaults }) => [
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
              position={positionFor(entry.vault)}
              pinned={isPinnedVault(entry)}
              rowIndex={i}
              groupSize={vaults.length}
              onManage={onManage}
              onPin={togglePinVault}
            />
          )),
        ])
      : section.flat.map((entry, i) => (
          <VaultRow
            key={entry.vault.vaultPubkey}
            entry={entry}
            grouped={false}
            connected={connected}
            position={positionFor(entry.vault)}
            pinned={isPinnedVault(entry)}
            rowIndex={i}
            groupSize={section.flat.length}
            onManage={onManage}
            onPin={togglePinVault}
          />
        ));

  // What the table says when it has no rows: still loading, the chain
  // unreachable, nothing on-chain at all, or vaults the filters hide.
  const liveEmptyText =
    chain.status === "loading"
      ? "Loading vaults…"
      : chain.status === "error"
        ? "Couldn't read vaults from the chain, retrying…"
        : liveGroups.length === 0
          ? "No vaults on-chain yet"
          : "No vaults match";

  return (
    <div className="mx-auto max-w-6xl px-6 pt-3 pb-16">
      {/* Below `sm` (~phone width) the table is hidden (the `hidden sm:block`
          guard below) and this prompt shows instead. Real mobile devices are
          redirected to /swap on load (MobileSwapRedirect), so in practice this
          is what a very narrow *desktop* window sees. From `sm` up (e.g. a
          laptop at half width) the full table shows and may scroll sideways. */}
      <div className="rounded-lg border border-border p-6 text-center text-muted-fg text-sm sm:hidden">
        The vaults table is best viewed on a wider screen. Widen your window or
        open Dropset on a desktop browser to browse vault pairs, leaders, APR
        and TVL.
      </div>
      {/* Center the toolbar + table as one block, sized to the wider of the
          two. The table is `w-full` so it fills the block when the toolbar is
          wider (the Pair column, the only one not `w-px`, takes the slack),
          and `min-w-max` so it never squeezes its columns: once the block
          hits `max-w-full` the table scrolls sideways instead. */}
      <div className="mx-auto hidden w-fit max-w-full sm:block">
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
            Read live from the chain. TVL is valued at each vault's reference
            price.
            {liveIndicative &&
              " Where a vault has no on-chain USD price (its book is dark), its TVL uses indicative token prices."}
          </p>
        </div>
        {/* Active URL pin (?base=&quote=&vault=) shown as removable chips on
            their own row, so a varying number of chips never changes the
            search/preview toolbar width (which the table aligns to). */}
        {(pin.base || pin.quote || pin.vault || pin.leader) && (
          <div className="mb-3 flex flex-wrap items-center gap-2">
            {(pin.base || pin.quote) && (
              <FilterChip
                label={`${pin.base || "·"} / ${pin.quote || "·"}`}
                // A vault number is per-market, so it clears with its pair.
                onClear={() => updatePin({ base: "", quote: "", vault: "" })}
              />
            )}
            {pin.vault && (
              <FilterChip
                label={`Vault #${pin.vault}`}
                onClear={() => updatePin({ vault: "" })}
              />
            )}
            {pin.leader && (
              <FilterChip
                label={`Leader ${pin.leader.length > LEADER_SLUG_MAX ? shortenMint(pin.leader) : pin.leader}`}
                onClear={() => updatePin({ leader: "" })}
              />
            )}
          </div>
        )}
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full min-w-max text-left text-sm">
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
                  sortKey="name"
                  label="Name"
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
                  className="w-px whitespace-nowrap border-border border-r bg-muted px-3 py-2 text-right font-medium normal-case"
                >
                  Manage
                </th>
                <VaultSortHeader
                  sortKey="tvl"
                  label="TVL"
                  sort={sort}
                  onToggle={toggleSort}
                  thClassName="w-px whitespace-nowrap"
                />
              </tr>
            </thead>
            <tbody>
              {live.flat.length > 0 ? (
                sectionRows(live)
              ) : (
                <MessageRow colSpan={colSpan} text={liveEmptyText} />
              )}
            </tbody>
          </table>
        </div>
      </div>
      {dialog && (
        <VaultActionDialog
          market={dialog.market}
          vault={currentVault(dialog.vault)}
          position={positionFor(dialog.vault)}
          open={true}
          onOpenChange={(open) => {
            if (open) return;
            setDialog(null);
            // A spent vault link leaves the pair pin but not the vault, so a
            // reload doesn't reopen a dialog the user closed.
            replaceUrlParams({ [VAULT_SEQ_PARAM]: null });
          }}
        />
      )}
    </div>
  );
}

// useSearchParams (read by the pin filter) must sit under a Suspense boundary.
export function VaultsView() {
  return (
    <Suspense fallback={null}>
      <VaultsInner />
    </Suspense>
  );
}
