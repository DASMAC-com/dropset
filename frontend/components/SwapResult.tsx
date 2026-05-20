"use client";

import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { stablecoinDecimals } from "@/lib/currencies";
import type { DflowSwapError, DflowSwapResult } from "@/lib/dflowSwap";
import { useSwapStore } from "@/lib/store";
import type { SwapStatus } from "@/lib/useDflowSwap";
import { ExternalLink } from "./icons";

const explorerTxUrl = (sig: string) => `https://explorer.solana.com/tx/${sig}`;

export function SwapResult({
  status,
  result,
  error,
}: {
  status: SwapStatus;
  result: DflowSwapResult | null;
  error: DflowSwapError | null;
}) {
  const fromStablecoin = useSwapStore((s) => s.from.stablecoin);
  const toStablecoin = useSwapStore((s) => s.to.stablecoin);

  if (status === "success" && result) {
    const inDec = stablecoinDecimals(fromStablecoin);
    const outDec = stablecoinDecimals(toStablecoin);
    const inStr = groupThousands(formatBaseAmount(result.inAmount, inDec));
    const outStr = groupThousands(formatBaseAmount(result.outAmount, outDec));
    return (
      <a
        href={explorerTxUrl(result.signature.toString())}
        target="_blank"
        rel="noreferrer"
        className="flex items-center justify-between gap-2 rounded-lg border border-accent-buy/40 bg-accent-buy/10 px-3 py-2 text-accent-buy text-sm transition-colors hover:bg-accent-buy/15"
      >
        <span className="truncate">
          Swapped {inStr} {fromStablecoin} → {outStr} {toStablecoin}
        </span>
        <ExternalLink size={14} className="shrink-0" />
      </a>
    );
  }

  if (status === "error" && error) {
    return (
      <div className="rounded-lg border border-accent-sell/40 bg-accent-sell/10 px-3 py-2 text-accent-sell text-sm">
        {error.kind === "rejected"
          ? "Swap cancelled."
          : `Swap failed: ${error.message}`}
      </div>
    );
  }

  return null;
}
