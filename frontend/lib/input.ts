// Decimal input sanitizers used by amount fields. Returned strings are
// safe to put back into a controlled <input>; callers can also Number-parse
// them once the user has stopped typing.

// Strip non-numeric characters, collapse multiple dots, and cap the
// fractional part at `decimals` places. Empty string is returned for inputs
// that contain no usable digit so the input clears rather than re-rendering
// stale junk.
export const sanitizeAmount = (raw: string, decimals: number): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    if (decimals === 0) v = v.slice(0, firstDot);
    else v = v.slice(0, firstDot + 1 + decimals);
  }
  return v;
};

// Same as sanitizeAmount but additionally clamps the value to [0, 100]
// (after sanitization) for percent fields and caps at 2 decimal places.
export const sanitizePercent = (raw: string): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    v = v.slice(0, firstDot + 1 + 2);
  }
  if (Number.parseFloat(v) > 100) v = "100";
  return v;
};

// Same as sanitizePercent but without the 100 clamp — slippage values can
// technically exceed 100% (e.g. for esoteric routes), and the underlying UI
// already caps via storage shape.
export const sanitizeSlippagePercent = (raw: string): string => {
  let v = raw.replace(/[^0-9.]/g, "");
  const firstDot = v.indexOf(".");
  if (firstDot !== -1) {
    v = v.slice(0, firstDot + 1) + v.slice(firstDot + 1).replace(/\./g, "");
    v = v.slice(0, firstDot + 1 + 2);
  }
  return v;
};

// Re-exported here so amount-formatting concerns live in one module from
// the consumer's POV — the implementation stays in lib/balance.ts where the
// related bigint helpers live.
export { groupThousands } from "./balance";
