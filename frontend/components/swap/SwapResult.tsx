"use client";

import { CircleAlert, CircleCheck, ExternalLink, X } from "@/components/icons";
import { stablecoinDecimals } from "@/lib/data/currencies";
import type { DflowSwapError } from "@/lib/dflow/dflowSwap";
import { explorerTxUrl } from "@/lib/explorer";
import { formatBaseAmount, groupThousands } from "@/lib/format/balance";
import type { CompletedSwap, SwapStatus } from "@/lib/hooks/useDflowSwap";

export function SwapResult({
  status,
  result,
  error,
  onClose,
}: {
  status: SwapStatus;
  result: CompletedSwap | null;
  error: DflowSwapError | null;
  onClose: () => void;
}) {
  if (status === "success" && result) {
    const inDec = stablecoinDecimals(result.fromStablecoin);
    const outDec = stablecoinDecimals(result.toStablecoin);
    const inStr = groupThousands(formatBaseAmount(result.inAmount, inDec));
    const outStr = groupThousands(formatBaseAmount(result.outAmount, outDec));
    return (
      <div
        role="status"
        aria-live="polite"
        className="flex items-center gap-2 rounded-xl border border-border bg-muted px-3 py-2 text-sm"
      >
        <CircleCheck
          size={18}
          className="shrink-0 text-accent-buy"
          aria-hidden
        />
        <span className="font-medium text-accent-buy">Order Succeeded</span>
        <a
          href={explorerTxUrl(result.signature.toString())}
          target="_blank"
          rel="noreferrer"
          className="inline-flex min-w-0 items-center gap-1 truncate text-muted-fg transition-colors hover:text-foreground"
        >
          <span className="truncate">
            {inStr} {result.fromStablecoin} → {outStr} {result.toStablecoin}
          </span>
          <ExternalLink size={12} className="shrink-0" />
        </a>
        <button
          type="button"
          onClick={onClose}
          aria-label="Dismiss"
          className="ml-auto flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-background hover:text-foreground"
        >
          <X size={14} />
        </button>
      </div>
    );
  }

  if (status === "error" && error) {
    const isCancel = error.kind === "rejected";
    return (
      <div
        role="alert"
        aria-live="assertive"
        className="flex items-center gap-2 rounded-xl border border-border bg-muted px-3 py-2 text-sm"
      >
        <CircleAlert
          size={18}
          className={`shrink-0 ${isCancel ? "text-muted-fg" : "text-accent-sell"}`}
          aria-hidden
        />
        <span
          className={`font-medium ${isCancel ? "text-foreground" : "text-accent-sell"}`}
        >
          {isCancel ? "Swap Cancelled" : "Swap Failed"}
        </span>
        {!isCancel && (
          <span className="min-w-0 truncate text-muted-fg">
            {error.message}
          </span>
        )}
        <button
          type="button"
          onClick={onClose}
          aria-label="Dismiss"
          className="ml-auto flex h-6 w-6 shrink-0 items-center justify-center rounded text-muted-fg hover:bg-background hover:text-foreground"
        >
          <X size={14} />
        </button>
      </div>
    );
  }

  return null;
}
