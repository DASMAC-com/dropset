import {
  currencyFlagUrl,
  currencyName,
  type IsoCurrencyCode,
} from "@/lib/currencies";

const CURRENCY_TINT: Record<string, { chip: string; border: string }> = {
  USD: { chip: "bg-emerald-500/15", border: "border-emerald-500/60" },
  EUR: { chip: "bg-sky-500/15", border: "border-sky-500/60" },
  GBP: { chip: "bg-indigo-500/15", border: "border-indigo-500/60" },
  JPY: { chip: "bg-rose-500/15", border: "border-rose-500/60" },
  AUD: { chip: "bg-amber-500/15", border: "border-amber-500/60" },
  BRL: { chip: "bg-green-500/15", border: "border-green-500/60" },
  CHF: { chip: "bg-red-500/15", border: "border-red-500/60" },
  MXN: { chip: "bg-orange-500/15", border: "border-orange-500/60" },
  NGN: { chip: "bg-teal-500/15", border: "border-teal-500/60" },
  ZAR: { chip: "bg-fuchsia-500/15", border: "border-fuchsia-500/60" },
};
const tintFor = (code: IsoCurrencyCode) =>
  CURRENCY_TINT[code] ?? { chip: "bg-muted", border: "border-border" };

export function CurrencyGroupHeader({ code }: { code: IsoCurrencyCode }) {
  const tint = tintFor(code);
  return (
    <div
      className={`mx-2 mb-1 flex items-center gap-2 border-b ${tint.border} px-0 py-1.5 text-muted-fg text-xs uppercase tracking-wide`}
    >
      <span
        aria-hidden
        className={`flex h-8 w-8 shrink-0 items-center justify-center overflow-hidden rounded-lg ${tint.chip}`}
      >
        {/* biome-ignore lint/performance/noImgElement: tiny static SVG, no optimization needed */}
        <img
          src={currencyFlagUrl(code)}
          alt=""
          aria-hidden
          width={24}
          height={18}
          className="rounded-sm shadow-sm"
        />
      </span>
      <span className="font-semibold text-foreground text-sm">{code}</span>
      <span className="text-muted-fg">·</span>
      <span>{currencyName(code)}</span>
    </div>
  );
}
