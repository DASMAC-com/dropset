// Decimal input sanitizers used by amount fields. Returned strings are
// safe to put back into a controlled <input>; callers can also Number-parse
// them once the user has stopped typing.

// Strip non-numeric characters, collapse extra dots, and cap the fractional
// part at `maxDecimals` places. Optionally clamp the parsed value to
// [0, maxValue]. Empty string is returned for inputs that contain no usable
// digit so the input clears rather than re-rendering stale junk.
//
// Shared core for sanitizeAmount / sanitizePercent / sanitizeSlippagePercent
// below — the previous three open-coded copies diverged only in the
// fractional cap and the optional value clamp.
const sanitizeDecimal = (
  raw: string,
  maxDecimals: number,
  maxValue?: number,
): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    if (maxDecimals === 0) v = v.slice(0, firstDot);
    else v = v.slice(0, firstDot + 1 + maxDecimals);
  }
  if (maxValue !== undefined && Number.parseFloat(v) > maxValue) {
    v = maxValue.toString();
  }
  return v;
};

export const sanitizeAmount = (raw: string, decimals: number): string =>
  sanitizeDecimal(raw, decimals);

// Percent fields are capped at 100 and 2 decimal places.
export const sanitizePercent = (raw: string): string =>
  sanitizeDecimal(raw, 2, 100);

// Same shape as sanitizePercent but without the 100 clamp — slippage values
// can technically exceed 100% (e.g. for esoteric routes), and the
// underlying UI already caps via storage shape.
export const sanitizeSlippagePercent = (raw: string): string =>
  sanitizeDecimal(raw, 2);

// Re-exported here so amount-formatting concerns live in one module from
// the consumer's POV — the implementation stays in lib/balance.ts where the
// related bigint helpers live.
export { groupThousands } from "./balance";
