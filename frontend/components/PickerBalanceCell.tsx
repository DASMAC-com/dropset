"use client";

import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { useAllBalances } from "@/lib/useAllBalances";
import { useInfoLookup } from "@/lib/useUsdQuote";

// Right-aligned wallet balance for a stablecoin row inside a token picker.
// Renders the atomic balance plus the USD equivalent stacked below it
// (Jupiter-style) when Jupiter has a price for the mint. Returns null when
// the wallet is disconnected or the initial balance fetch hasn't resolved
// — keeps the row free of placeholder noise during the brief load.
//
// All instances share one RPC fetch via `useAllBalances` and one Jupiter
// cache via `useInfoLookup`, so adding more consumers doesn't multiply
// network calls.
//
// No `ml-auto` — both pickers wrap StableTokenIdentity whose name column is
// flex-1, which already pushes this cell to the right. Omitting the auto
// margin keeps the cell reorderable against sibling controls.
const formatUsd = (n: number): string =>
  `$${n.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;

export function PickerBalanceCell({
  mint,
  decimals,
  symbol,
}: {
  mint: string;
  decimals: number;
  symbol: string;
}) {
  const { balanceFor } = useAllBalances();
  const lookup = useInfoLookup();
  const raw = balanceFor(mint);
  if (raw === undefined) return null;

  if (raw === null) {
    return (
      <span className="shrink-0 pl-2 text-muted-fg text-xs tabular-nums">
        —
      </span>
    );
  }

  const balanceText = groupThousands(formatBaseAmount(raw, decimals, 2));
  const price = lookup(mint)?.usdPrice;
  // Convert atomic bigint → float for the USD math. Stablecoin balances
  // never approach Number.MAX_SAFE_INTEGER in practice, so plain Number()
  // is fine; we'd want scaled bigint math only for unbounded-supply tokens.
  const usdText =
    typeof price === "number"
      ? formatUsd((Number(raw) / 10 ** decimals) * price)
      : null;

  return (
    <span className="flex shrink-0 flex-col items-end pl-2 text-xs leading-tight tabular-nums">
      <span className="text-muted-fg">
        {balanceText} {symbol}
      </span>
      {usdText !== null && (
        <span className="text-[10px] text-muted-fg/60">{usdText}</span>
      )}
    </span>
  );
}
