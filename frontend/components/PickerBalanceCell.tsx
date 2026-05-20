"use client";

import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { useAllBalances } from "@/lib/useAllBalances";

// Right-aligned wallet balance for a stablecoin row inside a token picker.
// Returns null (renders nothing) when the wallet is disconnected or the
// initial fetch hasn't resolved — keeps the row free of placeholder noise
// during the brief load before any data is available.
//
// All instances share one RPC fetch via `useAllBalances` — multiple cells
// rendered together (e.g. one per picker row) dedupe inside the hook, so
// adding more consumers doesn't multiply network calls.
export function PickerBalanceCell({
  mint,
  decimals,
}: {
  mint: string;
  decimals: number;
}) {
  const { balanceFor } = useAllBalances();
  const raw = balanceFor(mint);
  if (raw === undefined) return null;
  const text =
    raw === null ? "—" : groupThousands(formatBaseAmount(raw, decimals, 2));
  return (
    <span className="ml-auto shrink-0 pl-2 text-muted-fg text-xs tabular-nums">
      {text}
    </span>
  );
}
