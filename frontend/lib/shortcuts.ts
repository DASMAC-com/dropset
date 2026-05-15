"use client";

import { usePathname, useRouter } from "next/navigation";
import { useEffect } from "react";
import { emit } from "./events";

// Single source of truth for app-wide keyboard shortcuts, grouped by context.
// `global` shortcuts fire on every page; page-specific contexts (`swap`,
// `currencies`) layer on top. The combined set for a route must have no
// duplicate keys — see assertNoCollisions below.
export type ShortcutContext = "swap" | "currencies";

type Router = ReturnType<typeof useRouter>;

export type ShortcutRunContext = {
  router: Router;
};

export type ShortcutSpec = {
  key: string;
  description: string;
  run: (ctx: ShortcutRunContext) => void;
};

export const GLOBAL_SHORTCUTS: ShortcutSpec[] = [
  {
    key: "?",
    description: "Show this shortcuts list",
    run: () => emit("toggleHelp"),
  },
  {
    key: "w",
    description: "Connect or disconnect wallet",
    run: () => emit("toggleWallet"),
  },
];

export const SHORTCUTS_BY_CONTEXT: Record<ShortcutContext, ShortcutSpec[]> = {
  swap: [
    {
      key: "c",
      description: "Go to Currencies",
      run: ({ router }) => router.push("/currencies"),
    },
    {
      key: "f",
      description: "Open the From picker",
      run: () => emit("openPicker", "from"),
    },
    {
      key: "a",
      description: "Focus the From amount input",
      run: () => emit("focusFromAmount"),
    },
    {
      key: "t",
      description: "Open the To picker",
      run: () => emit("openPicker", "to"),
    },
    {
      key: "d",
      description: "Swap From and To direction",
      run: () => emit("swapSides"),
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
  ],
  currencies: [
    {
      key: "s",
      description: "Go to Swap",
      run: ({ router }) => router.push("/swap"),
    },
    {
      key: "/",
      description: "Focus the search input",
      run: () => emit("focusCurrenciesSearch"),
    },
    {
      key: "f",
      description: "Use the lone search result as From",
      run: () => emit("pickCurrencyOnlyResult", "from"),
    },
    {
      key: "t",
      description: "Use the lone search result as To",
      run: () => emit("pickCurrencyOnlyResult", "to"),
    },
  ],
};

const assertNoCollisions = (): void => {
  for (const ctx of Object.keys(SHORTCUTS_BY_CONTEXT) as ShortcutContext[]) {
    const seen = new Map<string, string>();
    for (const s of [...GLOBAL_SHORTCUTS, ...SHORTCUTS_BY_CONTEXT[ctx]]) {
      const k = s.key.toLowerCase();
      const prev = seen.get(k);
      if (prev) {
        throw new Error(
          `Keyboard shortcut collision in context "${ctx}": key "${s.key}" is used by both "${prev}" and "${s.description}". Resolve in lib/shortcuts.ts.`,
        );
      }
      seen.set(k, s.description);
    }
  }
};
assertNoCollisions();

export const shortcutsForPath = (pathname: string): ShortcutSpec[] => {
  const specific =
    pathname === "/swap"
      ? SHORTCUTS_BY_CONTEXT.swap
      : pathname === "/currencies"
        ? SHORTCUTS_BY_CONTEXT.currencies
        : [];
  return [...GLOBAL_SHORTCUTS, ...specific];
};

const isTextEditable = (target: EventTarget | null): boolean =>
  target instanceof HTMLInputElement ||
  target instanceof HTMLTextAreaElement ||
  (target instanceof HTMLElement && target.isContentEditable);

// Inputs marked with `data-shortcut-passthrough` let non-printable keys
// (Backspace, arrows, etc.) and digits/period reach the field, but route any
// other single-character key to the global shortcut handler so the user can
// trigger shortcuts without having to blur first.
const isPassthroughInput = (target: EventTarget | null): boolean =>
  target instanceof HTMLElement &&
  target.dataset.shortcutPassthrough === "true";

export function useKeyboardShortcuts(): void {
  const router = useRouter();
  const pathname = usePathname();
  useEffect(() => {
    const active = shortcutsForPath(pathname);
    const onKey = (e: KeyboardEvent) => {
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      const passthrough = isPassthroughInput(e.target);
      if (isTextEditable(e.target) && !passthrough) return;
      if (passthrough) {
        if (e.key.length !== 1) return;
        if (/[0-9.]/.test(e.key)) return;
      }
      const k = e.key.toLowerCase();
      const spec = active.find((s) => s.key === k);
      if (!spec) {
        if (passthrough) e.preventDefault();
        return;
      }
      e.preventDefault();
      spec.run({ router });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [router, pathname]);
}
