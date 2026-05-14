"use client";

import { useEffect, useState } from "react";
import { emit, useAppEvent } from "@/lib/events";
import { SHORTCUTS } from "@/lib/shortcuts";
import { Keyboard, X } from "./icons";

export function ShortcutsHelp() {
  const [open, setOpen] = useState(false);

  useAppEvent("toggleHelp", () => setOpen((v) => !v));

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  return (
    <>
      <button
        type="button"
        onClick={() => emit("toggleHelp")}
        aria-label="Show keyboard shortcuts"
        title="Keyboard shortcuts (/)"
        className="fixed right-4 bottom-4 z-40 flex h-10 w-10 items-center justify-center rounded-full border border-border bg-background/80 text-muted-fg shadow-sm backdrop-blur transition-colors hover:border-accent hover:text-accent"
      >
        <Keyboard size={18} />
      </button>

      {open && (
        // biome-ignore lint/a11y/noStaticElementInteractions: backdrop click dismiss; Escape and the close button cover keyboard paths
        // biome-ignore lint/a11y/useKeyWithClickEvents: same — keyboard dismissal handled by Escape and the close button
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-background/70 p-4 backdrop-blur-sm"
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
            <ul className="flex flex-col gap-2">
              {SHORTCUTS.map(({ key, displayKey, description }) => (
                <li
                  key={key}
                  className="flex items-center justify-between gap-3 text-sm"
                >
                  <span className="text-muted-fg">{description}</span>
                  <kbd className="shrink-0 rounded border border-border bg-muted px-2 py-0.5 font-mono text-foreground text-xs">
                    {displayKey ?? key}
                  </kbd>
                </li>
              ))}
            </ul>
            <p className="mt-4 text-muted-fg text-xs">
              Press <kbd className="font-mono">/</kbd> or{" "}
              <kbd className="font-mono">Esc</kbd> to close.
            </p>
          </div>
        </div>
      )}
    </>
  );
}
