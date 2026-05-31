"use client";

import { usePathname, useRouter } from "next/navigation";
import { useEffect } from "react";
import { emit } from "../events";

// Single source of truth for app-wide keyboard shortcuts, grouped by context.
// `global` shortcuts fire on every page; page-specific contexts (`swap`,
// `currencies`, `vaults`) layer on top, as does the page-aware Navigation set
// (see navShortcuts). The combined set for a route must have no duplicate keys
// — see assertNoCollisions below.
export type ShortcutContext = "swap" | "currencies" | "vaults";

// Section headings shown in the help dialog. Order shortcuts within each
// context so groups cluster together — the help dialog renders groups in
// first-appearance order.
export type ShortcutGroup =
  | "Navigation"
  | "General"
  | "Tokens"
  | "Trade"
  | "Map"
  | "Globe navigation"
  | "Search"
  | "Sort & display";

type Router = ReturnType<typeof useRouter>;

export type ShortcutRunContext = {
  router: Router;
};

export type ShortcutSpec = {
  key: string;
  description: string;
  group: ShortcutGroup;
  run: (ctx: ShortcutRunContext) => void;
};

export const GLOBAL_SHORTCUTS: ShortcutSpec[] = [
  {
    key: "?",
    description: "Show this shortcuts list",
    group: "General",
    run: () => emit("toggleHelp"),
  },
  {
    key: "w",
    description: "Connect or disconnect wallet",
    group: "General",
    run: () => emit("toggleWallet"),
  },
];

// Page-aware navigation: s / c / v jump to Swap / Currencies / Vaults from any
// page. Each is offered only on the pages it isn't already on, so a page's own
// initial stays free for a page action (e.g. `s` = slippage on /swap). These
// are layered ahead of the page-specific shortcuts in shortcutsForPath.
const NAV_TARGETS = [
  { context: "swap", key: "s", path: "/swap", label: "Swap" },
  { context: "currencies", key: "c", path: "/currencies", label: "Currencies" },
  { context: "vaults", key: "v", path: "/vaults", label: "Vaults" },
] as const satisfies {
  context: ShortcutContext;
  key: string;
  path: string;
  label: string;
}[];

const navShortcuts = (current: ShortcutContext | null): ShortcutSpec[] =>
  NAV_TARGETS.filter((t) => t.context !== current).map((t) => ({
    key: t.key,
    description: `Go to ${t.label}`,
    group: "Navigation",
    run: ({ router }) => router.push(t.path),
  }));

export const SHORTCUTS_BY_CONTEXT: Record<ShortcutContext, ShortcutSpec[]> = {
  swap: [
    {
      key: "f",
      description: "Open the From picker",
      group: "Tokens",
      run: () => emit("openPicker", "from"),
    },
    {
      key: "t",
      description: "Open the To picker",
      group: "Tokens",
      run: () => emit("openPicker", "to"),
    },
    {
      key: "d",
      description: "Swap From and To direction",
      group: "Tokens",
      run: () => emit("swapSides"),
    },
    {
      key: "a",
      description: "Focus the From amount input",
      group: "Trade",
      run: () => emit("focusFromAmount"),
    },
    {
      key: "m",
      description: "Use max From amount",
      group: "Trade",
      run: () => emit("applyMaxBalance"),
    },
    {
      key: "%",
      description: "Open the From balance % picker",
      group: "Trade",
      run: () => emit("openBalancePercent"),
    },
    {
      key: "s",
      description: "Open slippage settings",
      group: "Trade",
      run: () => emit("openSlippage"),
    },
    {
      key: "x",
      description: "Execute swap",
      group: "Trade",
      run: () => emit("executeSwap"),
    },
    {
      key: "r",
      description: "Reset the globe view",
      group: "Map",
      run: () => emit("resetGlobe"),
    },
    {
      key: "b",
      description: "Bird's-eye view of swap route",
      group: "Map",
      run: () => emit("focusRoute"),
    },
    {
      key: "e",
      description: "Toggle flag emojis on the map",
      group: "Map",
      run: () => emit("toggleFlags"),
    },
    {
      key: "p",
      description: "Toggle globe play/pause",
      group: "Map",
      run: () => emit("toggleSpin"),
    },
    {
      key: "=",
      description: "Zoom in",
      group: "Globe navigation",
      run: () => emit("zoomIn"),
    },
    {
      key: "-",
      description: "Zoom out",
      group: "Globe navigation",
      run: () => emit("zoomOut"),
    },
    {
      key: "i",
      description: "Pan north",
      group: "Globe navigation",
      run: () => emit("pan", "up"),
    },
    {
      key: "j",
      description: "Pan west",
      group: "Globe navigation",
      run: () => emit("pan", "left"),
    },
    {
      key: "k",
      description: "Pan south",
      group: "Globe navigation",
      run: () => emit("pan", "down"),
    },
    {
      key: "l",
      description: "Pan east",
      group: "Globe navigation",
      run: () => emit("pan", "right"),
    },
  ],
  currencies: [
    {
      key: "/",
      description: "Focus the search input",
      group: "Search",
      run: () => emit("focusCurrenciesSearch"),
    },
    {
      key: "f",
      description: "Use the lone search result as From",
      group: "Search",
      run: () => emit("pickCurrencyOnlyResult", "from"),
    },
    {
      key: "t",
      description: "Use the lone search result as To",
      group: "Search",
      run: () => emit("pickCurrencyOnlyResult", "to"),
    },
    {
      key: "g",
      description: "Toggle Group by currency",
      group: "Sort & display",
      run: () => emit("toggleGroupByCurrency"),
    },
    {
      // c (not t) for token: t is the To-picker key, and c reads as "currency"
      // — the parenthetical reinforces the mnemonic in the help dialog.
      key: "c",
      description: "Sort by (currency-backed) token",
      group: "Sort & display",
      run: () => emit("currenciesSort", "symbol"),
    },
    {
      key: "a",
      description: "Sort by mint address",
      group: "Sort & display",
      run: () => emit("currenciesSort", "mint"),
    },
    {
      key: "d",
      description: "Sort by 24h price change",
      group: "Sort & display",
      run: () => emit("currenciesSort", "priceChange24h"),
    },
    {
      key: "m",
      description: "Sort by market cap",
      group: "Sort & display",
      run: () => emit("currenciesSort", "mcap"),
    },
    {
      key: "l",
      description: "Sort by liquidity",
      group: "Sort & display",
      run: () => emit("currenciesSort", "liquidity"),
    },
    {
      key: "h",
      description: "Sort by holders",
      group: "Sort & display",
      run: () => emit("currenciesSort", "holderCount"),
    },
  ],
  vaults: [
    {
      key: "/",
      description: "Focus the search input",
      group: "Search",
      run: () => emit("focusVaultsSearch"),
    },
    {
      key: "g",
      description: "Toggle Group by pair",
      group: "Sort & display",
      run: () => emit("toggleGroupByPair"),
    },
    // Sort keys ordered to match the table's columns: Pair, Leader, Your
    // Position, APR, TVL, 24h Vol.
    {
      key: "p",
      description: "Sort by pair",
      group: "Sort & display",
      run: () => emit("vaultsSort", "pair"),
    },
    {
      key: "l",
      description: "Sort by leader",
      group: "Sort & display",
      run: () => emit("vaultsSort", "leader"),
    },
    {
      key: "y",
      description: "Sort by your position",
      group: "Sort & display",
      run: () => emit("vaultsSort", "position"),
    },
    {
      key: "a",
      description: "Sort by 24h APR",
      group: "Sort & display",
      run: () => emit("vaultsSort", "apr24h"),
    },
    {
      key: "t",
      description: "Sort by TVL",
      group: "Sort & display",
      run: () => emit("vaultsSort", "tvl"),
    },
    {
      key: "v",
      description: "Sort by 24h volume",
      group: "Sort & display",
      run: () => emit("vaultsSort", "volume24h"),
    },
  ],
};

const assertNoCollisions = (): void => {
  for (const ctx of Object.keys(SHORTCUTS_BY_CONTEXT) as ShortcutContext[]) {
    const seen = new Map<string, string>();
    for (const s of [
      ...navShortcuts(ctx),
      ...SHORTCUTS_BY_CONTEXT[ctx],
      ...GLOBAL_SHORTCUTS,
    ]) {
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

const CONTEXT_BY_PATH: Record<string, ShortcutContext> = {
  "/swap": "swap",
  "/currencies": "currencies",
  "/vaults": "vaults",
};

// Navigation leads (so it heads the help dialog), then the page-specific
// shortcuts, then the global wallet/help keys. On unknown routes there's no
// page context, so all three nav targets are offered.
export const shortcutsForPath = (pathname: string): ShortcutSpec[] => {
  const context = CONTEXT_BY_PATH[pathname] ?? null;
  const specific = context ? SHORTCUTS_BY_CONTEXT[context] : [];
  return [...navShortcuts(context), ...specific, ...GLOBAL_SHORTCUTS];
};

const isTextEditable = (target: EventTarget | null): boolean =>
  target instanceof HTMLInputElement ||
  target instanceof HTMLTextAreaElement ||
  (target instanceof HTMLElement && target.isContentEditable);

// Symbol keys that require Shift to produce (US keyboard). For these, a
// Shift modifier is expected and not a sign that the user meant a Shift+
// chord; for any other key we treat Shift as "this is a different chord"
// and refuse to match — so Shift+w doesn't fire the wallet shortcut.
const SHIFT_PRODUCED_SYMBOLS = new Set([
  "?",
  "%",
  "+",
  "/",
  ":",
  "@",
  "~",
  "<",
  ">",
]);

// Inputs marked with `data-shortcut-passthrough` are amount/percent fields
// that only accept digits, period, and minus — every other key is fair
// game for shortcut matching. This lets the user hit `m` for Max or `d`
// for direction-swap without first blurring the field with Escape.
// Non-printable keys (Escape, Tab, Backspace, arrows) reach the field so
// editing still works, and digit/decimal keys are explicitly held back.
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
      // Editable target without passthrough consumes every key.
      if (isTextEditable(e.target) && !passthrough) return;
      if (passthrough) {
        // Non-printable keys (Tab, Escape, Backspace, arrows…) belong to
        // the input — let it handle them.
        if (e.key.length !== 1) return;
        // Numeric input characters belong to the input too.
        if (/[0-9.,-]/.test(e.key)) return;
      }
      if (e.shiftKey && !SHIFT_PRODUCED_SYMBOLS.has(e.key)) return;
      // Match the key literally — no lowercase. Shift+letter would
      // produce a capital that won't match any registered shortcut.
      const spec = active.find((s) => s.key === e.key);
      if (!spec) {
        // While focused on a passthrough input, swallow any non-numeric
        // key that didn't match a shortcut so it doesn't slip through
        // and get inserted into the field anyway.
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
