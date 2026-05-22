"use client";

import * as Popover from "@radix-ui/react-popover";
import { type SyntheticEvent, useState } from "react";
import { COPY_FEEDBACK_DURATION_MS } from "@/lib/timings";
import { Check, Copy } from "./icons";

// Small inline copy button. After a click, the value is placed on the
// clipboard, the icon flips to a checkmark for 1.5 s, and a Radix Popover
// flashes "Copied: <value>" anchored right above the button so the
// confirmation lands at the click site (not in some corner of the viewport).
//
// Renders as a `<span role="button">` (via Radix's `asChild`) instead of a
// real `<button>` so the component can be safely nested inside a clickable
// parent (e.g. the picker row's select button). React/HTML disallow button
// inside button; span keeps the markup valid while preserving keyboard
// activation via the role + tabIndex + onKeyDown trio.
export function CopyButton({
  value,
  label,
}: {
  value: string;
  label?: string;
}) {
  const [copied, setCopied] = useState(false);

  const activate = async (e: SyntheticEvent) => {
    // Stop the event from bubbling to clickable parents (e.g. the picker
    // row's select button) so copying doesn't also switch the selected
    // token.
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), COPY_FEEDBACK_DURATION_MS);
    } catch {
      // clipboard API unavailable — silently ignore
    }
  };

  return (
    <Popover.Root open={copied}>
      <Popover.Trigger asChild>
        {/* biome-ignore lint/a11y/useSemanticElements: rendered as a span (not <button>) so the component can nest inside a clickable picker-row button without producing invalid <button>-in-<button> markup. */}
        <span
          role="button"
          tabIndex={0}
          onClick={activate}
          onKeyDown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              void activate(e);
            }
          }}
          title={copied ? "Copied!" : `Copy ${label ?? "value"}`}
          className="inline-flex shrink-0 cursor-pointer items-center gap-1 rounded p-1 text-muted-fg outline-none hover:bg-muted hover:text-accent focus-visible:ring-1 focus-visible:ring-accent"
        >
          {copied ? <Check size={12} /> : <Copy size={12} />}
        </span>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          side="top"
          sideOffset={4}
          onOpenAutoFocus={(e) => e.preventDefault()}
          className="z-[100] rounded-md border border-border bg-background px-2 py-1 text-foreground text-xs shadow-lg"
        >
          Copied:{" "}
          <span className="break-all font-mono text-foreground">{value}</span>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}
