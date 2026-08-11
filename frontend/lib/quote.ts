import type { RouterVenue } from "@dropset/sdk";

// The swap UI's quote state, shared by every quoting hook so the panel is
// agnostic to which route produced the numbers. Lives here rather than in one
// of the hooks because both `useRouterQuote` (best route) and `useEclobQuote`
// (Dropset only) produce it and neither owns the other.

export type QuoteStatus =
  | "idle"
  | "loading"
  | "ok"
  | "error"
  | "rateLimited"
  | "skipped";

// Flat shape (not a discriminated union) on purpose: outAmount/inAmount
// persist across status transitions so the to-side keeps the previous quote
// visible while a new fetch is in flight. A strict
// { status: "loading"; outAmount: null } | { status: "ok"; outAmount: bigint }
// union would force consumers to either hide the prior amount during every
// refetch (flicker) or duplicate it onto the loading variant (bookkeeping).
// The mint-pair fields plus `hasQuote` are what consumers use to detect a
// stale-but-still-shown previous result.
export type QuoteState = {
  status: QuoteStatus;
  outAmount: bigint | null;
  inAmount: bigint | null;
  // The mints this quote was fetched for. Consumers should compare these
  // against the *current* store mints to detect a stale cached quote.
  inputMint: string | null;
  outputMint: string | null;
  priceImpactPct: string | null;
  slippageBps: number | null;
  // The platform fee, in bps, this quote was actually computed with — the
  // number to *display*, which is not always the configured `PLATFORM_FEE.bps`.
  // The eCLOB route clamps the configured rate to the market's on-chain
  // `max_platform_fee`, so a market with a lower (or zero) ceiling is quoted,
  // and charged, at less than the config asks for. Reporting the config value
  // instead would show a fee the user isn't paying — or invent one on a market
  // where fees are turned off entirely.
  //
  // `null` means "no platform fee applies to this quote". The DFlow route
  // reports the configured rate, since its fee is settled by DFlow against no
  // on-chain ceiling; its separate precondition (the fee wallet's ATA must
  // already exist) is resolved when the quote is fetched.
  platformFeeBps: number | null;
  // Which venue the router picked. Null until a quote lands.
  venue: RouterVenue | null;
  hasQuote: boolean;
  error: string | null;
};

export const INITIAL_QUOTE: QuoteState = {
  status: "idle",
  outAmount: null,
  inAmount: null,
  inputMint: null,
  outputMint: null,
  priceImpactPct: null,
  slippageBps: null,
  platformFeeBps: null,
  venue: null,
  hasQuote: false,
  error: null,
};

// Format an atomic BigInt amount back to a decimal string. Trailing zeros
// after the decimal point are trimmed; integer-only values render without
// a trailing dot.
export const formatAtomic = (n: bigint, decimals: number): string => {
  if (decimals === 0) return n.toString();
  const s = n.toString().padStart(decimals + 1, "0");
  const i = s.length - decimals;
  const intPart = s.slice(0, i);
  const fracPart = s.slice(i).replace(/0+$/, "");
  return fracPart ? `${intPart}.${fracPart}` : intPart;
};
