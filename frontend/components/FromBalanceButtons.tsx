"use client";

import { useSplToken, useWalletConnection } from "@solana/react-hooks";
import { stablecoinDecimals, stablecoinMint } from "@/lib/currencies";
import { useSwapStore } from "@/lib/store";

// Convert a raw base-unit bigint into a normalized decimal string respecting
// the token's `decimals`. Trailing zeros are stripped; "0.0" collapses to "0".
const formatBaseAmount = (base: bigint, decimals: number): string => {
  const s = base.toString();
  if (decimals === 0) return s;
  const padded = s.padStart(decimals + 1, "0");
  const intPart = padded.slice(0, -decimals).replace(/^0+(?=\d)/, "");
  const fracPart = padded.slice(-decimals).replace(/0+$/, "");
  return fracPart ? `${intPart}.${fracPart}` : intPart;
};

const FRACTIONS: { label: string; divisor: bigint }[] = [
  { label: "25%", divisor: 4n },
  { label: "50%", divisor: 2n },
  { label: "Max", divisor: 1n },
];

export function FromBalanceButtons() {
  const stablecoin = useSwapStore((s) => s.from.stablecoin);
  const setAmount = useSwapStore((s) => s.setAmount);
  const { connected } = useWalletConnection();
  const mint = stablecoinMint(stablecoin);
  const decimals = stablecoinDecimals(stablecoin);
  const { balance, status } = useSplToken(mint);

  const base = balance?.exists ? balance.amount : 0n;
  const disabled = !connected || status !== "ready" || base === 0n;

  const apply = (divisor: bigint) => {
    if (base === 0n) return;
    const portion = divisor === 1n ? base : base / divisor;
    setAmount(formatBaseAmount(portion, decimals));
  };

  return (
    <div className="ml-auto flex shrink-0 items-center gap-1">
      {FRACTIONS.map(({ label, divisor }) => (
        <button
          key={label}
          type="button"
          disabled={disabled}
          onClick={() => apply(divisor)}
          title={
            connected
              ? `Use ${label} of your ${stablecoin} balance`
              : "Connect wallet to use balance"
          }
          className="rounded border border-border bg-background px-2 py-1 font-medium text-muted-fg text-sm transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40 disabled:hover:border-border disabled:hover:text-muted-fg"
        >
          {label}
        </button>
      ))}
    </div>
  );
}
