"use client";

import { useSplToken, useWalletConnection } from "@solana/react-hooks";
import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { useAppEvent } from "@/lib/events";
import { AUTO_DETECT_TOKEN_PROGRAM } from "@/lib/splTokenOptions";
import { Wallet } from "./icons";

export function WalletBalance({ stablecoin }: { stablecoin: string }) {
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balance, status, refresh } = useSplToken(
    mint,
    AUTO_DETECT_TOKEN_PROGRAM,
  );

  // After a swap lands the on-chain balances move for both the from- and
  // to-mints. Refresh twice — once now, once ~1.5 s later — because the
  // RPC's account state can lag the tx's confirmation status by a slot or
  // two, and an immediate refetch sometimes still sees pre-swap data.
  useAppEvent("swapSucceeded", () => {
    void refresh();
    window.setTimeout(() => {
      void refresh();
    }, 1500);
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
