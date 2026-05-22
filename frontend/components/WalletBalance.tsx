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
  const { balanceFor, isReady, error } = useAllBalances();
  const raw = balanceFor(mint);

  if (!connected) return null;
  // Surface the failure as "?" so the user knows the balance didn't load
  // — silently showing nothing makes it look like they have no funds.
  if (error) {
    return (
      <span
        className="flex items-center gap-1 text-muted-fg"
        title={`Couldn't load ${stablecoin} balance: ${error}`}
      >
        <Wallet size={14} aria-hidden />
        <span>? {stablecoin}</span>
      </span>
    );
  }
  // Hide entirely until the initial fetch resolves — showing "—" / "0"
  // before we know would be misleading.
  if (!isReady) return null;

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
