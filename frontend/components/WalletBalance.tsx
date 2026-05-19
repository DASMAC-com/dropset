"use client";

import { useSplToken, useWalletConnection } from "@solana/react-hooks";
import { formatBaseAmount, groupThousands } from "@/lib/balance";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { Wallet } from "./icons";

export function WalletBalance({ stablecoin }: { stablecoin: string }) {
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balance, status } = useSplToken(mint);

  if (
    !connected ||
    status !== "ready" ||
    !balance?.exists ||
    balance.amount === 0n
  ) {
    return null;
  }

  const formatted = groupThousands(formatBaseAmount(balance.amount, decimals));

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
