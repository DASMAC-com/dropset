"use client";

import * as Popover from "@radix-ui/react-popover";
import { type ReactNode, useState } from "react";
import { Info } from "@/components/icons";
import { Z_TOOLTIP } from "@/lib/ui/dialog";

// A small info "i" with a tooltip bubble. Built on Radix Popover (already a
// dep) rather than a CSS hover-only span so it is actually accessible:
// - the trigger is a real <button>, keyboard-focusable, with the text as its
//   aria-label so screen readers get the explanation even unopened;
// - the bubble opens on hover AND focus, and closes on blur/Escape/outside;
// - the content is Portaled, so it can't be clipped by an overflow-y-auto
//   dialog or a sticky table header (the old absolute bubble was).
export function InfoTooltip({
  label,
  size = 12,
  side = "bottom",
  className = "",
}: {
  label: ReactNode;
  size?: number;
  side?: "top" | "bottom";
  className?: string;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger
        type="button"
        aria-label={typeof label === "string" ? label : "More info"}
        onMouseEnter={() => setOpen(true)}
        onMouseLeave={() => setOpen(false)}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        className={`inline-flex items-center rounded text-muted-fg outline-none transition-colors hover:text-foreground focus-visible:text-foreground ${className}`}
      >
        <Info size={size} aria-hidden />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side={side}
          align="end"
          sideOffset={4}
          // It's a tooltip, not a focus trap — don't pull focus on open.
          onOpenAutoFocus={(e) => e.preventDefault()}
          className={`${Z_TOOLTIP} w-56 whitespace-normal rounded-md border border-border bg-background px-2 py-1.5 text-left font-normal text-[11px] text-muted-fg normal-case shadow-lg`}
        >
          {label}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
