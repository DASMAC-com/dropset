"use client";

import { useKeyboardShortcuts } from "@/lib/ui/shortcuts";

// Tiny client-only mount point for the global shortcut listener. Lives at the
// top of the page tree so the listener installs once regardless of which
// panel is in focus.
export function KeyboardShortcuts(): null {
  useKeyboardShortcuts();
  return null;
}
