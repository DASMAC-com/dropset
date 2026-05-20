"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { useAllBalances } from "@/lib/useAllBalances";
import { Wallet } from "./icons";

export function WalletBalance({ stablecoin }: { stablecoin: string }) {
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balanceFor, isReady } = useAllBalances();
  const raw = balanceFor(mint);

  // Hide entirely while disconnected or until the initial fetch resolves —
  // showing "—" / "0" before we know would be misleading.
  if (!connected || !isReady) return null;

  // null → no associated token account (display "—"). 0n → ATA exists with
  // zero balance (display "0"). Positive bigint → formatted number.
  const display =
    raw === null
      ? "—"
      : groupThousands(formatBaseAmount(raw ?? 0n, decimals, 2));

  return (
    <span
      className="flex items-center gap-1 text-muted-fg"
      title={`Your ${stablecoin} balance`}
    >
      <Wallet size={14} aria-hidden />
      <span>
        {display} {stablecoin}
      </span>
    </span>
  );
}
