"use client";

import { useEffect, useRef, useState } from "react";
import { FLASH_DURATION_MS } from "./timings";

// Track value changes across renders and briefly mark a cell as "just
// updated" so the user can see what a refresh touched. Layered alongside
// <NumberFlow> to give two cues: digit-roll animation + a brief
// background flash. Direction is intentionally not encoded — neutral
// flashes compose with whatever tone the surrounding cell already uses
// (up/down columns, status colors, etc.) instead of stacking signals.
//
// The visual state is derived from a `flashUntil` timestamp instead of a
// boolean useState, so any re-render (including a parent reorder)
// recomputes `flashing` from the wall clock. An earlier boolean version
// could get wedged on `true`: if `setFlashing(false)` happened to be
// queued in the same React tick as a parent state change, the boolean
// update could be missed and the cell stayed highlighted. The timer
// here just exists to force one extra re-render at the dismiss boundary;
// the truth is always `Date.now() < flashUntil`.
//
// Accepts `unknown` so the same hook can drive number-valued cells (USD
// prices), bigint-valued cells (atomic-unit swap quotes), or anything
// else identity-comparable via Object.is.
export const useFlashOnChange = (value: unknown): boolean => {
  const prev = useRef<unknown>(value);
  const initialized = useRef(value != null);
  const flashUntil = useRef(0);
  const timer = useRef<number | null>(null);
  const [, force] = useState(0);

  useEffect(() => {
    if (Object.is(value, prev.current)) return;
    prev.current = value;
    if (!initialized.current) {
      if (value != null) initialized.current = true;
      return;
    }
    flashUntil.current = Date.now() + FLASH_DURATION_MS;
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      force((x) => x + 1);
    }, FLASH_DURATION_MS);
    force((x) => x + 1);
  }, [value]);

  useEffect(
    () => () => {
      if (timer.current !== null) window.clearTimeout(timer.current);
    },
    [],
  );

  return Date.now() < flashUntil.current;
};

export const flashBg = (on: boolean): string => (on ? "bg-muted-fg/15" : "");

// Bulk variant for rows that need to flash multiple cells from a single
// data source. Tracks each value independently against its previous tick
// and returns a tuple of booleans in the same order. Saves N hook calls
// per row (and N timers) when a card or table row exposes many flashing
// cells off the same fetch.
export const useFlashOnChanges = <T extends readonly unknown[]>(
  values: T,
): { [K in keyof T]: boolean } => {
  const prev = useRef<T>(values);
  const initialized = useRef<boolean[]>(values.map((v) => v != null));
  const flashUntil = useRef<number[]>(values.map(() => 0));
  const timers = useRef<(number | null)[]>(values.map(() => null));
  const [, force] = useState(0);

  // biome-ignore lint/correctness/useExhaustiveDependencies: each tuple
  //   slot is compared via Object.is below; tracking individual scalars in
  //   the deps would require a stable tuple-key string here that this
  //   effect computes itself.
  useEffect(() => {
    let anyChange = false;
    for (let i = 0; i < values.length; i++) {
      const v = values[i];
      if (Object.is(v, prev.current[i])) continue;
      anyChange = true;
      (prev.current as unknown as unknown[])[i] = v;
      if (!initialized.current[i]) {
        if (v != null) initialized.current[i] = true;
        continue;
      }
      flashUntil.current[i] = Date.now() + FLASH_DURATION_MS;
      const existing = timers.current[i];
      if (existing !== null) window.clearTimeout(existing);
      timers.current[i] = window.setTimeout(() => {
        timers.current[i] = null;
        force((x) => x + 1);
      }, FLASH_DURATION_MS);
    }
    if (anyChange) force((x) => x + 1);
  }, [values]);

  useEffect(
    () => () => {
      for (const t of timers.current) {
        if (t !== null) window.clearTimeout(t);
      }
    },
    [],
  );

  const now = Date.now();
  return values.map((_, i) => now < (flashUntil.current[i] ?? 0)) as {
    [K in keyof T]: boolean;
  };
};
