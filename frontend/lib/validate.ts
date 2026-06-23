// Tiny hand-rolled validators for the four external response shapes we
// trust the least: DFlow /quote, DFlow /order, Jupiter /tokens/v2/search,
// and Solana RPC's jsonParsed token-account payload. These boundaries
// previously cast directly to a hand-typed shape and then BigInt-coerced
// fields without checking — a malformed body would surface to the UI as a
// generic "Network error" with no diagnostic.

export const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null;

export const isString = (v: unknown): v is string => typeof v === "string";

export const isNumber = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);

// Coerce a decimal-string into bigint. BigInt() throws SyntaxError on any
// non-integer input (including scientific notation, decimal points, trailing
// whitespace mixed with characters); we surface those as a single typed
// reason rather than letting them propagate as a raw TypeError. BigInt()
// already tolerates surrounding whitespace (`BigInt(" 123 ") === 123n`), so
// no pre-trim is needed.
//
// Every caller is a DFlow swap amount, which is an unsigned atomic figure, so
// we reject a negative value at the boundary rather than rely on each consumer
// re-checking — a negative `outAmount` would otherwise render as a negative
// "To" figure in the UI. The error message also doesn't echo the raw value — a
// malformed upstream response could include sensitive-looking data we don't
// want surfaced to the user; the field name is enough to diagnose.
export const parseBigIntString = (value: unknown, field: string): bigint => {
  if (!isString(value)) {
    throw new ValidationError(`${field} missing or not a string`);
  }
  let parsed: bigint;
  try {
    parsed = BigInt(value);
  } catch {
    throw new ValidationError(`${field} is not a valid integer`);
  }
  if (parsed < 0n) {
    throw new ValidationError(`${field} must be non-negative`);
  }
  return parsed;
};

export class ValidationError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ValidationError";
  }
}

export type ParsedDflowQuote = {
  inAmount: bigint;
  outAmount: bigint;
  priceImpactPct: string | null;
  slippageBps: number | null;
};

export const parseDflowQuote = (raw: unknown): ParsedDflowQuote => {
  if (!isObject(raw)) {
    throw new ValidationError("quote response is not an object");
  }
  return {
    inAmount: parseBigIntString(raw.inAmount, "quote.inAmount"),
    outAmount: parseBigIntString(raw.outAmount, "quote.outAmount"),
    priceImpactPct: isString(raw.priceImpactPct) ? raw.priceImpactPct : null,
    slippageBps: isNumber(raw.slippageBps) ? raw.slippageBps : null,
  };
};

export type ParsedDflowOrder = {
  transaction: string;
  inAmount: bigint;
  outAmount: bigint;
};

export const parseDflowOrder = (raw: unknown): ParsedDflowOrder => {
  if (!isObject(raw)) {
    throw new ValidationError("order response is not an object");
  }
  if (!isString(raw.transaction) || raw.transaction.length === 0) {
    throw new ValidationError("order.transaction missing or empty");
  }
  return {
    transaction: raw.transaction,
    inAmount: parseBigIntString(raw.inAmount, "order.inAmount"),
    outAmount: parseBigIntString(raw.outAmount, "order.outAmount"),
  };
};

// Jupiter `/tokens/v2/search` row. Only the fields the UI actually reads
// are checked at this boundary; the rest pass through as-is for callers
// that want to project additional fields later. Returns null when the
// row's `id` is missing or non-string — those rows are skipped by
// callers rather than included with bogus data.
export type ParsedJupiterRow = {
  id: string;
  usdPrice: number | null;
  priceChange24h: number | null;
  mcap: number | null;
  liquidity: number | null;
  holderCount: number | null;
  stats24h: {
    priceChange: number | null;
    buyVolume: number | null;
    sellVolume: number | null;
  };
};

const nullableNumber = (v: unknown): number | null => (isNumber(v) ? v : null);

const parseJupiterRow = (raw: unknown): ParsedJupiterRow | null => {
  if (!isObject(raw)) return null;
  if (!isString(raw.id)) return null;
  const stats = isObject(raw.stats24h) ? raw.stats24h : {};
  return {
    id: raw.id,
    usdPrice: nullableNumber(raw.usdPrice),
    priceChange24h: nullableNumber(raw.priceChange24h),
    mcap: nullableNumber(raw.mcap),
    liquidity: nullableNumber(raw.liquidity),
    holderCount: nullableNumber(raw.holderCount),
    stats24h: {
      priceChange: nullableNumber(stats.priceChange),
      buyVolume: nullableNumber(stats.buyVolume),
      sellVolume: nullableNumber(stats.sellVolume),
    },
  };
};

// Validate a Jupiter /tokens/v2/search response: the body must be an array
// (we already check that at the boundary), and each row must have at
// minimum a string `id`. Rows that fail the per-row check are dropped
// rather than failing the whole batch — partial usable data beats none.
export const parseJupiterSearchResponse = (
  raw: unknown,
): ParsedJupiterRow[] | null => {
  if (!Array.isArray(raw)) return null;
  const out: ParsedJupiterRow[] = [];
  for (const row of raw) {
    const parsed = parseJupiterRow(row);
    if (parsed !== null) out.push(parsed);
  }
  return out;
};

// jsonParsed token account: `data.parsed.info.tokenAmount.amount` is a
// stringified bigint, present whenever the RPC recognized the address as a
// SPL/Token-2022 account. Anything else (missing, unparsable) means the
// address isn't a token account in the expected encoding — surface that
// distinctly from "0n balance".
export const parseTokenAccountAmount = (data: unknown): bigint | null => {
  if (!isObject(data)) return null;
  const parsed = data.parsed;
  if (!isObject(parsed)) return null;
  const info = parsed.info;
  if (!isObject(info)) return null;
  const tokenAmount = info.tokenAmount;
  if (!isObject(tokenAmount)) return null;
  const amount = tokenAmount.amount;
  if (!isString(amount)) return null;
  try {
    return BigInt(amount);
  } catch {
    return null;
  }
};
