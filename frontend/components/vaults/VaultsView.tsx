"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { useSearchParams } from "next/navigation";
import { Suspense, useCallback, useEffect, useMemo, useState } from "react";
import { SearchBox } from "@/components/ui/SearchBox";
import { SortableHeader, type SortState } from "@/components/ui/SortableHeader";
import { VaultActionDialog } from "@/components/vaults/dialog/VaultActionDialog";
import { FilterChip } from "@/components/vaults/FilterChip";
import { VaultRow } from "@/components/vaults/VaultRow";
import {
  FxGroupHeading,
  MessageRow,
} from "@/components/vaults/VaultTableHeadings";
import { shortenMint } from "@/lib/data/currencies";
import { positionPnl } from "@/lib/data/pnl";
import type { VaultPosition } from "@/lib/data/positions";
import {
  findVaultBySeq,
  type GroupedVault,
  groupVaultMarkets,
  isVaultPinned,
  type MetricKey,
  type Vault,
  type VaultMarket,
  vaultRefPrice,
} from "@/lib/data/vaults";
import { realizedVault } from "@/lib/eclob/vaultTx";
import { useAppEvent } from "@/lib/events";
import { useChainVaults } from "@/lib/hooks/useChainVaults";
import {
  replaceUrlParams,
  useVaultPin,
  VAULT_SEQ_PARAM,
} from "@/lib/ui/swapUrl";
import { type ArrangedSection, arrangeVaultsTable } from "@/lib/vaults/table";

// The table shows only what the chain gives. APR, 24h volume and the pair's
// 24h figures need trade history, so they return with the indexer rather than
// rendering blank here; a depositor's PnL stays on their position, marked at
// the vault's reference price (vaultRefPrice).

// Pin the generic shared header to this table's metric keys so the literal
// `sortKey` props type-check against `sort` / `onToggle`.
const VaultSortHeader = SortableHeader<MetricKey>;

// A `leader` pin longer than this is treated as a full pubkey and shortened in
// its chip; a shorter value is a hand-typed prefix and shown verbatim.
const LEADER_SLUG_MAX = 12;

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
  // The URL pin filter (`?base=&quote=&leader=`, or `&vault=`); see useVaultPin.
  const [pin, updatePin] = useVaultPin();

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
  // Memoized so the table's useMemo below has a stable dependency.
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

  const toggleSort = (key: MetricKey) =>
    setSort((prev) => {
      if (!prev || prev.key !== key) return { key, direction: "desc" };
      if (prev.direction === "desc") return { key, direction: "asc" };
      return null;
    });

  // Keyboard shortcuts (see lib/ui/shortcuts.ts → vaults context).
  useAppEvent("toggleGroupByPair", () => setGroupByPair((g) => !g));
  useAppEvent("vaultsSort", (key) => toggleSort(key));

  const live = useMemo(
    () => arrangeVaultsTable(liveGroups, { query, pin, sort, positionValue }),
    [liveGroups, query, pin, sort, positionValue],
  );

  const onManage = (market: VaultMarket, vault: Vault) =>
    setDialog({ market, vault });

  // The pin "crosshair" on each row toggles the URL filter to that vault,
  // keyed by its leader's address (never its number), so it also shows any
  // other vault the same leader leads in the pair.
  const isPinnedVault = (e: GroupedVault) =>
    isVaultPinned(pin, e.market, e.vault);
  const togglePinVault = (e: GroupedVault) =>
    updatePin(
      isPinnedVault(e)
        ? { base: "", quote: "", vault: "", leader: "" }
        : {
            base: e.market.base,
            quote: e.market.quote,
            vault: "",
            leader: e.vault.leader,
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
  // unreachable, nothing onchain at all, or vaults the filters hide.
  const liveEmptyText =
    chain.status === "loading"
      ? "Loading vaults…"
      : chain.status === "error"
        ? "Couldn't read vaults from the chain, retrying…"
        : liveGroups.length === 0
          ? "No vaults onchain yet"
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
              " Where a vault has no onchain USD price (its book is dark), its TVL uses indicative token prices."}
          </p>
        </div>
        {/* Active URL pin (?base=&quote=&leader=, or &vault=) shown as removable chips on
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
