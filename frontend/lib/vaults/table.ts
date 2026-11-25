import {
  cmpPairCode,
  type FxPairGroup,
  type GroupedVault,
  groupMetric,
  type MetricKey,
  type Vault,
  type VaultPin,
  vaultLabel,
  vaultMetric,
  vaultNumberLabel,
  vaultPinMatches,
} from "@/lib/data/vaults";
import { compareSortValues, type SortDir, type SortState } from "@/lib/ui/sort";

// The Vaults table's view-model: which rows show, and in what order. Pure, so
// the view only feeds it state; the connected wallet's position value arrives
// as a function because it depends on wallet state (see MetricKey).

// Shared null-sinking, case-insensitive comparator (see SortableHeader).
const cmpMetric = compareSortValues;

// The connected user's position value in a vault, 0 when they hold none.
export type PositionValue = (vault: Vault) => number;

// Substring match of an already trimmed, lowercased query across the pair's
// FX label / currency names and the vault's tokens, name, vault number and
// leader address. An empty query matches everything.
export const matchesQuery = (
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

// The table's rows, filtered and sorted both ways.
export type ArrangedSection = {
  groups: { group: FxPairGroup; vaults: GroupedVault[] }[];
  flat: GroupedVault[];
};

// Filter + sort the groups: grouped (each group's vaults filtered and sorted,
// then the groups sorted, dropping any left empty) and flat (every vault,
// filtered and sorted as one list).
export function arrangeSection(
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

// A vault row's value under a sort column.
export const vaultSortValue = (
  gv: GroupedVault,
  key: MetricKey,
  positionValue: PositionValue,
): number | string | null => {
  if (key === "position") return positionValue(gv.vault);
  if (key === "name") return vaultLabel(gv.vault);
  if (key === "pair") return gv.market.label;
  return vaultMetric(gv, key);
};

// A pair group's value under a sort column.
export const groupSortValue = (
  g: FxPairGroup,
  key: MetricKey,
  positionValue: PositionValue,
): number | string | null => {
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
};

// The view state the table's rows follow: the search query (raw, as typed),
// the URL pin, the column sort and the wallet's position values.
export type VaultsTableState = {
  query: string;
  pin: VaultPin;
  sort: SortState<MetricKey>;
  positionValue: PositionValue;
};

// The table's rows for its current state. With no column sort chosen, the
// pairs read alphabetically by base currency code (cmpPairCode) and each
// pair's vaults by TVL, largest first.
export function arrangeVaultsTable(
  groups: FxPairGroup[],
  state: VaultsTableState,
): ArrangedSection {
  const { pin, sort, positionValue } = state;
  const q = state.query.trim().toLowerCase();
  const keep = (group: FxPairGroup, entry: GroupedVault) =>
    matchesQuery(q, group, entry) &&
    vaultPinMatches(pin, entry.market, entry.vault);
  const byVault =
    (key: MetricKey, direction: SortDir) =>
    (a: GroupedVault, b: GroupedVault) =>
      cmpMetric(
        vaultSortValue(a, key, positionValue),
        vaultSortValue(b, key, positionValue),
        direction,
      );
  if (sort === null) {
    const byTvl = byVault("tvl", "desc");
    return arrangeSection(
      groups,
      keep,
      cmpPairCode,
      (a, b) => cmpPairCode(a.market, b.market) || byTvl(a, b),
    );
  }
  const { key, direction } = sort;
  const cmpGroup = (a: FxPairGroup, b: FxPairGroup) =>
    cmpMetric(
      groupSortValue(a, key, positionValue),
      groupSortValue(b, key, positionValue),
      direction,
    );
  return arrangeSection(groups, keep, cmpGroup, byVault(key, direction));
}
