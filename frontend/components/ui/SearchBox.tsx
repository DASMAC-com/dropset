"use client";

import { useRef, useState } from "react";
import { Search, X } from "@/components/icons";
import { useAppEvent } from "@/lib/events";

// Shared search input for the data tables on /currencies and /vaults. Owns the
// presentational shell (border, magnifier, `/`-vs-`Esc` kbd hint, clear button)
// and the focus/Escape/Enter interaction; the parent owns the query state and
// any side effects (URL sync, filtering) via the callbacks. The `/` shortcut
// reaches the input through `focusEvent` (see lib/ui/shortcuts.ts).
export function SearchBox({
  value,
  onValueChange,
  onClear,
  onCommit,
  placeholder,
  focusEvent,
  widthClassName = "w-56",
}: {
  value: string;
  onValueChange: (value: string) => void;
  onClear: () => void;
  onCommit?: () => void;
  placeholder: string;
  focusEvent: "focusCurrenciesSearch" | "focusVaultsSearch";
  widthClassName?: string;
}) {
  const [focused, setFocused] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useAppEvent(focusEvent, () => {
    inputRef.current?.focus();
    inputRef.current?.select();
  });

  return (
    <div
      className={`flex h-9 ${widthClassName} items-center gap-2 rounded-md border border-border bg-muted px-3`}
    >
      <Search size={14} className="shrink-0 text-muted-fg" />
      <input
        ref={inputRef}
        type="text"
        value={value}
        onChange={(e) => onValueChange(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => {
          setFocused(false);
          onCommit?.();
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.preventDefault();
            inputRef.current?.blur();
          } else if (e.key === "Enter") {
            e.preventDefault();
            onCommit?.();
            inputRef.current?.blur();
          }
        }}
        placeholder={placeholder}
        aria-label={placeholder}
        className="min-w-0 flex-1 bg-transparent text-foreground text-sm outline-none placeholder:text-muted-fg"
      />
      <kbd
        aria-hidden
        title={focused ? "Press Esc to exit search" : "Press / to focus search"}
        className="hidden shrink-0 rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[10px] text-muted-fg sm:inline-block"
      >
        {focused ? "Esc" : "/"}
      </kbd>
      {value && (
        <button
          type="button"
          onClick={onClear}
          aria-label="Clear search"
          className="flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-background hover:text-foreground"
        >
          <X size={14} />
        </button>
      )}
    </div>
  );
}
