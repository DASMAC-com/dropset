import Link from "next/link";
import { HelpCircle } from "@/components/icons";

// Small shared anchor that deep-links into /currencies with the stablecoin's
// symbol pre-populated in the search. Uses Next's <Link> (not a plain <a>) so
// the navigation stays client-side and the module-level Zustand store
// survives — a hard navigation would re-initialize the store to defaults and
// wipe the user's in-progress swap selection.
//
// Rendered inline next to the shortened mint in `StableTokenIdentity` — and
// in the dropdown picker that wraps the identity in a row-select `<button>`,
// clicks would otherwise bubble up and switch the user's selected token.
// `stopPropagation` keeps clicks on the link link-only.
export function TokenInfoLink({
  symbol,
  className = "",
}: {
  symbol: string;
  className?: string;
}) {
  return (
    <Link
      href={`/currencies?symbol=${encodeURIComponent(symbol)}`}
      title={`More info about ${symbol}`}
      onClick={(e) => e.stopPropagation()}
      className={`flex shrink-0 items-center rounded p-0.5 text-muted-fg hover:bg-muted hover:text-accent ${className}`}
    >
      <HelpCircle size={12} />
    </Link>
  );
}
