"use client";

import {
  type LucideIcon,
  type LucideProps,
  ArrowUpDown as RawArrowUpDown,
  Check as RawCheck,
  ChevronDown as RawChevronDown,
  Compass as RawCompass,
  Copy as RawCopy,
  Crosshair as RawCrosshair,
  ExternalLink as RawExternalLink,
  Flag as RawFlag,
  Keyboard as RawKeyboard,
  Minus as RawMinus,
  Pause as RawPause,
  Play as RawPlay,
  Plus as RawPlus,
  Search as RawSearch,
  Settings2 as RawSettings2,
  X as RawX,
} from "lucide-react";

// Browser extensions like Dark Reader and Grammarly inject inline-style
// attributes onto SVGs before React hydrates, producing benign-but-noisy
// hydration mismatches. Wrapping each Lucide icon so the rendered <svg>
// always has suppressHydrationWarning silences those warnings — the prop
// passes through Lucide's prop spread to the underlying svg element.
const safe = (Icon: LucideIcon): LucideIcon => {
  const Wrapped = (props: LucideProps) => (
    <Icon {...props} suppressHydrationWarning />
  );
  Wrapped.displayName = `Safe(${Icon.displayName ?? "Icon"})`;
  return Wrapped as unknown as LucideIcon;
};

export const ArrowUpDown = safe(RawArrowUpDown);
export const Check = safe(RawCheck);
export const ChevronDown = safe(RawChevronDown);
export const Compass = safe(RawCompass);
export const Copy = safe(RawCopy);
export const Crosshair = safe(RawCrosshair);
export const ExternalLink = safe(RawExternalLink);
export const Flag = safe(RawFlag);
export const Keyboard = safe(RawKeyboard);
export const Minus = safe(RawMinus);
export const Pause = safe(RawPause);
export const Play = safe(RawPlay);
export const Plus = safe(RawPlus);
export const Search = safe(RawSearch);
export const Settings2 = safe(RawSettings2);
export const X = safe(RawX);
