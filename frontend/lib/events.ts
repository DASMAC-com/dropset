"use client";

import { useEffect, useRef } from "react";
import type { Side } from "./store";

// Module-level fire-and-forget event bus for cross-component signals
// (keyboard shortcuts, toolbar buttons, etc.). Use this instead of pulse
// signals in the store when there's no actual state to persist — just an
// action to fan out. Adding a new event = one line in `AppEvents` and one
// `useAppEvent` call in the consumer.
export type PanDirection = "up" | "down" | "left" | "right";

export type AppEvents = {
  openPicker: Side;
  focusFromAmount: undefined;
  resetGlobe: undefined;
  focusRoute: undefined;
  swapSides: undefined;
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
};

type Handler<K extends keyof AppEvents> = (payload: AppEvents[K]) => void;

// Internally we store handlers as a single untyped fn; the public emit /
// useAppEvent surface keeps the per-event types correct.
type AnyHandler = (payload: unknown) => void;
const listeners: Partial<Record<keyof AppEvents, Set<AnyHandler>>> = {};

export function emit<K extends keyof AppEvents>(
  name: K,
  ...args: AppEvents[K] extends undefined ? [] : [AppEvents[K]]
): void {
  const set = listeners[name];
  if (!set) return;
  const payload = args[0];
  for (const h of set) h(payload);
}

// Subscribe to an event for the lifetime of the component. The handler is
// stored in a ref so callers can pass inline arrow functions without churning
// the subscription on every render.
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
