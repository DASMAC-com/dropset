import { X } from "@/components/icons";

// A removable filter chip for an active URL pin.
export function FilterChip({
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
