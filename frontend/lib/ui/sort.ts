// The data tables' sort state and value comparator, kept free of React so a
// table's pure view-model (lib/vaults/table.ts) can use them; the shared
// header (components/ui/SortableHeader) re-exports them.

export type SortDir = "asc" | "desc";
export type SortState<K extends string> = { key: K; direction: SortDir } | null;

// Order two sort values for a column. Nulls (missing market data, zero-TVL APR,
// etc.) always sink to the bottom regardless of direction; strings compare
// case-insensitively, with digit runs as numbers (so "#10" follows "#5").
// Shared by /vaults and /currencies so the two tables sort identically.
export const compareSortValues = (
  a: number | string | null,
  b: number | string | null,
  direction: SortDir,
): number => {
  if (a === null && b === null) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  if (typeof a === "string" && typeof b === "string") {
    const c = a.localeCompare(b, undefined, {
      sensitivity: "base",
      numeric: true,
    });
    return direction === "desc" ? -c : c;
  }
  return direction === "desc"
    ? (b as number) - (a as number)
    : (a as number) - (b as number);
};
