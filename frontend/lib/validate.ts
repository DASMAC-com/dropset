// Tiny hand-rolled validators for the three external response shapes we
// trust the least: DFlow /quote, DFlow /order, and Solana RPC's jsonParsed
// token-account payload. These boundaries previously cast directly to a
// hand-typed shape and then BigInt-coerced fields without checking — a
// malformed body would surface to the UI as a generic "Network error" with
// no diagnostic.

export const isObject = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null;

export const isString = (v: unknown): v is string => typeof v === "string";

export const isNumber = (v: unknown): v is number =>
  typeof v === "number" && Number.isFinite(v);

// Coerce a decimal-string into bigint. BigInt() throws SyntaxError on any
// non-integer input (including scientific notation, decimal points, trailing
// whitespace mixed with characters); we surface those as a single typed
// reason rather than letting them propagate as a raw TypeError.
export const parseBigIntString = (value: unknown, field: string): bigint => {
  if (!isString(value)) {
    throw new ValidationError(`${field} missing or not a string`);
  }
  try {
    return BigInt(value);
  } catch {
    throw new ValidationError(`${field} is not a valid integer: ${value}`);
  }
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
