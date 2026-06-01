"use client";

import type { ReactNode } from "react";
import { Info } from "@/components/icons";

// A small info "i" with a CSS hover tooltip. Native `title` tooltips are
// unreliable here (they don't fire on the inline SVG and can't be styled), so
// we render our own group-hover bubble — the pattern the sortable column
// headers use, extracted so the deposit "Auto" pill and any future "i" share
// one implementation.
//
// `align` anchors the bubble's left/right edge to the icon (use "right" near a
// container's right edge so it opens inward). `side` opens it below ("bottom")
// or above ("top") the icon.
export function InfoTooltip({
  label,
  size = 12,
  align = "right",
  side = "bottom",
  className = "",
}: {
  label: ReactNode;
  size?: number;
  align?: "left" | "right";
  side?: "top" | "bottom";
  className?: string;
}) {
  const position = [
    side === "bottom" ? "top-full mt-1" : "bottom-full mb-1",
    align === "right" ? "right-0" : "left-0",
  ].join(" ");
  return (
    <span className={`group relative inline-flex items-center ${className}`}>
      <Info
        size={size}
        className="text-muted-fg transition-colors group-hover:text-foreground"
      />
      <span
        role="tooltip"
        className={`pointer-events-none absolute z-[90] w-56 whitespace-normal rounded-md border border-border bg-background px-2 py-1.5 text-left font-normal text-[11px] text-muted-fg normal-case opacity-0 shadow-lg transition-opacity duration-150 group-hover:opacity-100 ${position}`}
      >
        {label}
      </span>
    </span>
  );
}
