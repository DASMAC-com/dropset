"use client";

import { ArrowUpDown, ChevronDown, ChevronUp } from "@/components/icons";
import { InfoTooltip } from "@/components/ui/InfoTooltip";

// Shared sortable column header for the data tables on /currencies and
// /vaults. Generic over the table's sort-key union so each table keeps its own
// strongly-typed keys. Renders a right-aligned sort button (with an
// asc/desc/inactive chevron) and an optional info icon whose hover tooltip
// explains the column.

export type SortDir = "asc" | "desc";
export type SortState<K extends string> = { key: K; direction: SortDir } | null;

export function SortableHeader<K extends string>({
  sortKey,
  label,
  sort,
  onToggle,
  info,
  thClassName = "",
  align = "right",
}: {
  sortKey: K;
  label: string;
  sort: SortState<K>;
  onToggle: (key: K) => void;
  info?: string;
  // Extra classes on the <th> — e.g. "w-px whitespace-nowrap" to snap the
  // column to its content width.
  thClassName?: string;
  // Header alignment — "left" for text columns (token, leader), "right" for
  // numeric ones. Matches the cell alignment.
  align?: "left" | "right";
}) {
  const active = sort?.key === sortKey;
  const Icon = !active
    ? ArrowUpDown
    : sort.direction === "desc"
      ? ChevronDown
      : ChevronUp;
  const ariaSort = !active
    ? "none"
    : sort.direction === "desc"
      ? "descending"
      : "ascending";
  return (
    <th
      scope="col"
      aria-sort={ariaSort}
      className={`sticky top-14 z-20 border-border border-r bg-muted p-0 last:border-r-0 ${thClassName}`}
    >
      <div
        className={`flex items-center gap-1 px-3 py-2 ${align === "left" ? "justify-start" : "justify-end"}`}
      >
        <button
          type="button"
          onClick={() => onToggle(sortKey)}
          className={`flex cursor-pointer select-none items-center gap-1 text-right font-medium outline-none transition-colors focus:outline-none focus-visible:outline-none ${active ? "text-foreground" : "text-muted-fg hover:text-foreground"}`}
        >
          {label}
          <Icon size={12} aria-hidden />
        </button>
        {info && <InfoTooltip label={info} />}
      </div>
    </th>
  );
}
