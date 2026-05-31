// cspell:word Fron
"use client";

import { useEffect, useRef } from "react";
import type { Side } from "./store";

// Module-level fire-and-forget event bus for cross-component signals
// (keyboard shortcuts, toolbar buttons, etc.). Use this instead of pulse
// state in the store when there's no actual state to persist — just an
// action to fan out. Adding a new event = one line in `AppEvents` and one
// `useAppEvent` call in the consumer.
//
// Type safety: `emit` and `useAppEvent` are both generic over `keyof
// AppEvents`, so the event name string IS type-checked at every call site
// — a typo (`emit("focusFronAmount")`) fails to compile, and the payload
// shape on each side is inferred from the AppEvents map. The EVENTS
// constant exposed below is a stylistic convenience for find-usages: a
// reference to `EVENTS.swapSides` lights up across the codebase the way
// a string literal doesn't, and is identical at runtime.
export type PanDirection = "up" | "down" | "left" | "right";

export type AppEvents = {
  openPicker: Side;
  focusFromAmount: undefined;
  applyMaxBalance: undefined;
  openBalancePercent: undefined;
  swapSucceeded: undefined;
  openSlippage: undefined;
  resetGlobe: undefined;
  focusRoute: undefined;
  swapSides: undefined;
  executeSwap: undefined;
  toggleSpin: undefined;
  toggleFlags: undefined;
  toggleHelp: undefined;
  openWalletModal: undefined;
  toggleWallet: undefined;
  zoomIn: undefined;
  zoomOut: undefined;
  pan: PanDirection;
  focusCurrenciesSearch: undefined;
  pickCurrencyOnlyResult: Side;
  toggleGroupByCurrency: undefined;
  currenciesSort: CurrenciesSortKey;
  toggleGroupByPair: undefined;
  vaultsSort: VaultsSortKey;
  focusVaultsSearch: undefined;
};

export type CurrenciesSortKey =
  | "symbol"
  | "mint"
  | "priceChange24h"
  | "volume24h"
  | "mcap"
  | "liquidity"
  | "holderCount";

export type VaultsSortKey =
  | "apr24h"
  | "tvl"
  | "volume24h"
  | "position"
  | "leader"
  | "pair";

// Mirror of AppEvents keys as an importable constant. Use this at call
// sites when you want refactor-friendly references (rename one entry and
// every consumer breaks at compile time).
export const EVENTS: { [K in keyof AppEvents]: K } = {
  openPicker: "openPicker",
  focusFromAmount: "focusFromAmount",
  applyMaxBalance: "applyMaxBalance",
  openBalancePercent: "openBalancePercent",
  swapSucceeded: "swapSucceeded",
  openSlippage: "openSlippage",
  resetGlobe: "resetGlobe",
  focusRoute: "focusRoute",
  swapSides: "swapSides",
  executeSwap: "executeSwap",
  toggleSpin: "toggleSpin",
  toggleFlags: "toggleFlags",
  toggleHelp: "toggleHelp",
  openWalletModal: "openWalletModal",
  toggleWallet: "toggleWallet",
  zoomIn: "zoomIn",
  zoomOut: "zoomOut",
  pan: "pan",
  focusCurrenciesSearch: "focusCurrenciesSearch",
  pickCurrencyOnlyResult: "pickCurrencyOnlyResult",
  toggleGroupByCurrency: "toggleGroupByCurrency",
  currenciesSort: "currenciesSort",
  toggleGroupByPair: "toggleGroupByPair",
  vaultsSort: "vaultsSort",
  focusVaultsSearch: "focusVaultsSearch",
};

type Handler<K extends keyof AppEvents> = (payload: AppEvents[K]) => void;

// Internally we store handlers as a single untyped fn; the public emit /
// useAppEvent surface keeps the per-event types correct.
type AnyHandler = (payload: unknown) => void;
const listeners: Partial<Record<keyof AppEvents, Set<AnyHandler>>> = {};

/**
 * Fire-and-forget event dispatch. The event `name` is type-checked against
 * `AppEvents`; the second argument is required iff the event has a
 * non-undefined payload type.
 */
export function emit<K extends keyof AppEvents>(
  name: K,
  ...args: AppEvents[K] extends undefined ? [] : [AppEvents[K]]
): void {
  const set = listeners[name];
  if (!set) return;
  const payload = args[0];
  // Each handler is wrapped so a thrower in one subscriber doesn't prevent
  // the remaining handlers from running.
  for (const h of set) {
    try {
      h(payload);
    } catch (e) {
      console.error(`[events] handler for "${String(name)}" threw:`, e);
    }
  }
}

/**
 * Subscribe to an event for the lifetime of the component. The handler is
 * stored in a ref so callers can pass inline arrow functions without
 * churning the subscription on every render.
 */
export function useAppEvent<K extends keyof AppEvents>(
  name: K,
  handler: Handler<K>,
): void {
  const ref = useRef(handler);
  ref.current = handler;
  useEffect(() => {
    const wrapped: AnyHandler = (arg) => ref.current(arg as AppEvents[K]);
    const set = listeners[name] ?? new Set<AnyHandler>();
    listeners[name] = set;
    set.add(wrapped);
    return () => {
      set.delete(wrapped);
    };
  }, [name]);
}
