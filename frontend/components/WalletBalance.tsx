"use client";

import { useSplToken, useWalletConnection } from "@solana/react-hooks";
import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { Wallet } from "./icons";

export function WalletBalance({ stablecoin }: { stablecoin: string }) {
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balance, status, refresh } = useSplToken(mint);

  // After a swap lands the on-chain balances move for both the from- and
  // to-mints. Each WalletBalance instance refreshes its own mint; SWR
  // de-dupes when multiple subscribers share a key.
  useAppEvent("swapSucceeded", () => {
    void refresh();
  });

  // Hide entirely while disconnected or until the balance fetch resolves —
  // showing "0" before we know would be misleading. Once `ready`, render
  // even when the user has no balance for this token (shows "0 USDC") so
  // the row stays visually consistent across token switches.
  if (!connected || status !== "ready") return null;

  const amount = balance?.exists ? balance.amount : 0n;
  const formatted = groupThousands(formatBaseAmount(amount, decimals));

  return (
    <span
      className="flex items-center gap-1 text-muted-fg"
      title={`Your ${stablecoin} balance`}
    >
      <Wallet size={14} aria-hidden />
      <span>
        {formatted} {stablecoin}
      </span>
    </span>
  );
}
