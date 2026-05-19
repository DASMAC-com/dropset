"use client";

import * as Popover from "@radix-ui/react-popover";
import { useState } from "react";
import { Check, Copy } from "./icons";

// Small inline copy button. After a click, the value is placed on the
// clipboard, the icon flips to a checkmark for 1.5 s, and a Radix Popover
// flashes "Copied: <value>" anchored right above the button so the
// confirmation lands at the click site (not in some corner of the viewport).
export function CopyButton({
  value,
  label,
}: {
  value: string;
  label?: string;
}) {
  const [copied, setCopied] = useState(false);

  const onClick = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // clipboard API unavailable — silently ignore
    }
  };

  return (
    <Popover.Root open={copied}>
      <Popover.Trigger
        type="button"
        onClick={onClick}
        title={copied ? "Copied!" : `Copy ${label ?? "value"}`}
        className="inline-flex shrink-0 items-center gap-1 rounded p-1 text-muted-fg hover:bg-muted hover:text-accent"
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
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
