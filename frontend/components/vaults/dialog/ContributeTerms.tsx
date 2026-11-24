import type { Vault } from "@/lib/data/vaults";

// The vault's performance fee as a percent label, e.g. "10%" for 100,000 ppm.
const perfFeeLabel = (ppm: number): string =>
  `${(ppm / 10_000).toLocaleString("en-US", { maximumFractionDigits: 2 })}%`;

// What a depositor is buying and when it is quoted, in plain words under the
// deposit form: no share math, and the risk line last. The fee line reads the
// vault's own rate.
export function ContributeTerms({ vault }: { vault: Vault }) {
  const feePpm = vault.chain.perfFeeRatePpm;
  const terms = [
    "Your deposit is quoted after the leader's next re-center.",
    "Withdraw at any time, paid in both assets.",
    feePpm > 0
      ? `The leader takes a ${perfFeeLabel(feePpm)} performance fee on the vault's gains.`
      : "The leader takes no performance fee.",
  ];
  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border bg-muted px-3 py-3 text-muted-fg text-xs">
      <ul className="flex list-disc flex-col gap-1 pl-4">
        {terms.map((t) => (
          <li key={t}>{t}</li>
        ))}
      </ul>
      <p className="border-border border-t pt-1.5 text-foreground">
        The leader controls the ladder and the fee. You can leave at any time.
        You are taking a risk.
      </p>
    </div>
  );
}
