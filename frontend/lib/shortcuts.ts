"use client";

import { useEffect } from "react";
import { emit } from "./events";
import { useSwapStore } from "./store";

// Single source of truth for app-wide keyboard shortcuts. Each entry maps a
// case-insensitive key to a side effect. Add new shortcuts here; nothing else
// needs to change.
export type ShortcutSpec = {
  // The KeyboardEvent.key value to match (lowercased). For arrow keys this is
  // `ArrowUp` / `ArrowDown` etc.; the help dialog uses `displayKey` when
  // present so the listing can show a glyph instead of a verbose name.
  key: string;
  displayKey?: string;
  description: string;
  run: () => void;
};

export const SHORTCUTS: ShortcutSpec[] = [
  {
    key: "f",
    description: "Open the From picker",
    run: () => emit("openPicker", "from"),
  },
  {
    key: "t",
    description: "Open the To picker",
    run: () => emit("openPicker", "to"),
  },
  {
    key: "d",
    description: "Swap From and To direction",
    run: () => useSwapStore.getState().swapSides(),
  },
  {
    key: "r",
    description: "Reset the globe view",
    run: () => emit("resetGlobe"),
  },
  {
    key: "s",
    description: "Focus on swap route",
    run: () => emit("focusRoute"),
  },
  {
    key: "e",
    description: "Toggle flag emojis on the map",
    run: () => emit("toggleFlags"),
  },
  {
    key: "p",
    description: "Toggle globe play/pause",
    run: () => emit("toggleSpin"),
  },
  {
    key: "=",
    description: "Zoom in",
    run: () => emit("zoomIn"),
  },
  {
    key: "-",
    description: "Zoom out",
    run: () => emit("zoomOut"),
  },
  {
    key: "i",
    description: "Pan north",
    run: () => emit("pan", "up"),
  },
  {
    key: "j",
    description: "Pan west",
    run: () => emit("pan", "left"),
  },
  {
    key: "k",
    description: "Pan south",
    run: () => emit("pan", "down"),
  },
  {
    key: "l",
    description: "Pan east",
    run: () => emit("pan", "right"),
  },
  {
    key: "/",
    description: "Show this shortcuts list",
    run: () => emit("toggleHelp"),
  },
];

const isTextEditable = (target: EventTarget | null): boolean =>
  target instanceof HTMLInputElement ||
  target instanceof HTMLTextAreaElement ||
  (target instanceof HTMLElement && target.isContentEditable);

export function useKeyboardShortcuts(): void {
  useEffect(() => {
    const byKey = new Map(SHORTCUTS.map((s) => [s.key.toLowerCase(), s]));
    const onKey = (e: KeyboardEvent) => {
      if (isTextEditable(e.target)) return;
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const spec = byKey.get(e.key.toLowerCase());
      if (!spec) return;
      e.preventDefault();
      spec.run();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}
