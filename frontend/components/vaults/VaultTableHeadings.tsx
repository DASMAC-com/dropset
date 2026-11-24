import NumberFlow from "@number-flow/react";
import { FlagPair } from "@/components/ui/Flag";
import type { FxPairGroup } from "@/lib/data/vaults";
import { FORMATS } from "@/lib/format/formats";

// FX-pair heading spanning the table, mirroring the per-currency headings on
// /currencies: two flags + "EUR / USD", with the pair's summed TVL
// and vault count to the right.
export function FxGroupHeading({
  group,
  colSpan,
}: {
  group: FxPairGroup;
  colSpan: number;
}) {
  const count = group.vaults.length;
  return (
    <tr className="bg-background">
      <td colSpan={colSpan} className="px-3 pt-8 pb-3">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
          <FlagPair
            base={group.baseFlagUrl}
            quote={group.quoteFlagUrl}
            size={48}
          />
          <span className="font-semibold text-foreground text-xl">
            {group.label}
          </span>
          <span className="flex items-center gap-3 font-mono text-muted-fg text-xs tabular-nums">
            <span>
              TVL <NumberFlow value={group.tvl} format={FORMATS.usdCompact} />
            </span>
            <span>
              {count} {count === 1 ? "vault" : "vaults"}
            </span>
          </span>
        </div>
      </td>
    </tr>
  );
}

// A full-width message row in the table body.
export function MessageRow({
  colSpan,
  text,
}: {
  colSpan: number;
  text: string;
}) {
  return (
    <tr>
      <td
        colSpan={colSpan}
        className="px-3 py-6 text-center text-muted-fg text-sm"
      >
        {text}
      </td>
    </tr>
  );
}
