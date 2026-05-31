"use client";

import { ArrowUpDown, ChevronDown, ChevronUp, Info } from "@/components/icons";

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
  return (
    <th
      scope="col"
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
          <Icon size={12} />
        </button>
        {info && (
          <span className="group relative inline-flex items-center">
            <Info
              size={12}
              className="text-muted-fg transition-colors group-hover:text-foreground"
            />
            {/* whitespace-normal resets the nowrap a snug `w-px
                whitespace-nowrap` header inherits onto its descendants, so the
                tooltip wraps inside its width instead of stretching out on one
                line. */}
            <span
              role="tooltip"
              className="pointer-events-none absolute top-full right-0 z-30 mt-1 w-56 whitespace-normal rounded-md border border-border bg-background px-2 py-1.5 text-left font-normal text-[11px] text-muted-fg normal-case opacity-0 shadow-lg transition-opacity duration-150 group-hover:opacity-100"
            >
              {info}
            </span>
          </span>
        )}
      </div>
    </th>
  );
}
