"use client";

import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import { X } from "@/components/icons";
import { useAppEvent } from "@/lib/events";
import {
  type ShortcutGroup,
  type ShortcutSpec,
  shortcutsForPath,
} from "@/lib/ui/shortcuts";

// Bucket shortcuts by group, preserving the order each group first appears in.
const groupShortcuts = (
  shortcuts: ShortcutSpec[],
): Array<[ShortcutGroup, ShortcutSpec[]]> => {
  const buckets = new Map<ShortcutGroup, ShortcutSpec[]>();
  for (const s of shortcuts) {
    const existing = buckets.get(s.group);
    if (existing) existing.push(s);
    else buckets.set(s.group, [s]);
  }
  return Array.from(buckets.entries());
};

export function ShortcutsHelp() {
  const [open, setOpen] = useState(false);
  const pathname = usePathname();
  const groups = groupShortcuts(shortcutsForPath(pathname));

  useAppEvent("toggleHelp", () => setOpen((v) => !v));

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  if (!open) return null;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click dismiss; Escape and the close button cover keyboard paths
    // biome-ignore lint/a11y/useKeyWithClickEvents: same — keyboard dismissal handled by Escape and the close button
    <div
      className="fixed inset-0 z-[100] flex items-start justify-center bg-background/70 px-4 pt-6 pb-4 backdrop-blur-sm"
      onClick={() => setOpen(false)}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: stopPropagation only — keyboard interaction happens inside the dialog content */}
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="shortcuts-help-title"
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-sm rounded-xl border border-border bg-background p-6 text-left shadow-lg"
      >
        <div className="mb-4 flex items-center justify-between gap-3">
          <h2
            id="shortcuts-help-title"
            className="font-semibold text-foreground text-lg"
          >
            Keyboard shortcuts
          </h2>
          <button
            type="button"
            onClick={() => setOpen(false)}
            aria-label="Close"
            className="flex h-7 w-7 items-center justify-center rounded text-muted-fg hover:bg-muted hover:text-foreground"
          >
            <X size={16} />
          </button>
        </div>
        <div className="flex flex-col gap-4">
          {groups.map(([group, items]) => (
            <section key={group} className="flex flex-col gap-2">
              <h3 className="font-medium text-foreground text-xs uppercase tracking-wide">
                {group}
              </h3>
              <ul className="flex flex-col gap-2">
                {items.map(({ key, description }) => (
                  <li
                    key={key}
                    className="flex items-center justify-between gap-3 text-sm"
                  >
                    <span className="text-muted-fg">{description}</span>
                    <kbd className="shrink-0 rounded border border-border bg-muted px-2 py-0.5 font-mono text-foreground text-xs">
                      {key}
                    </kbd>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
        <p className="mt-4 text-muted-fg text-xs">
          Press <kbd className="font-mono">?</kbd> or{" "}
          <kbd className="font-mono">Esc</kbd> to close.
        </p>
      </div>
    </div>
  );
}
