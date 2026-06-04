"use client";

import { useWalletConnection } from "@solana/react-hooks";
import { Loader2, Wallet } from "@/components/icons";
import { stablecoinDecimals, stablecoinMint } from "@/lib/data/currencies";
import { formatBalanceDisplay } from "@/lib/format/balance";
import { useAllBalances } from "@/lib/hooks/useAllBalances";

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
  // Balances are chunked out to the RPC, so the initial fetch takes a beat.
  // Show a spinner in the wallet-icon slot rather than "—" / "0" (misleading)
  // or nothing (looks like the row is broken / unconnected).
  if (!isReady) {
    return (
      <span
        className="flex items-center gap-1 text-muted-fg"
        title={`Loading your ${stablecoin} balance…`}
      >
        <Loader2 size={14} className="animate-spin" aria-hidden />
        <span>{stablecoin}</span>
      </span>
    );
  }

  // null → no associated token account (display "—"). 0n → ATA exists with
  // zero balance (display "0"). Positive bigint → formatted number. The
  // ?? 0n covers undefined (still loading), but that case is unreachable
  // here because `isReady` is true above.
  const display =
    raw === null ? "—" : formatBalanceDisplay(raw ?? 0n, decimals);

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
