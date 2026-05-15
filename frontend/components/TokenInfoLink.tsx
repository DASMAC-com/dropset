import { HelpCircle } from "./icons";

// Small shared anchor that deep-links into /currencies with the stablecoin's
// symbol pre-populated in the search. Used by both the swap-page token picker
// and the on-map country picker so behavior + styling stay aligned.
export function TokenInfoLink({
  symbol,
  className = "",
}: {
  symbol: string;
  className?: string;
}) {
  return (
    <a
      href={`/currencies?q=${encodeURIComponent(symbol)}`}
      title={`More info about ${symbol}`}
      className={`flex shrink-0 items-center rounded p-1 text-muted-fg hover:bg-muted hover:text-accent ${className}`}
    >
      <HelpCircle size={12} />
    </a>
  );
}
