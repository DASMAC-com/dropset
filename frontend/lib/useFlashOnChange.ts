"use client";

import { useEffect, useRef, useState } from "react";

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
    flashUntil.current = Date.now() + 1000;
    if (timer.current !== null) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = null;
      force((x) => x + 1);
    }, 1000);
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
