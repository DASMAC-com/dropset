// Convert a raw base-unit bigint into a normalized decimal string respecting
// the token's `decimals`. Trailing zeros are stripped; "0.0" collapses to "0".
export const formatBaseAmount = (base: bigint, decimals: number): string => {
  const s = base.toString();
  if (decimals === 0) return s;
  const padded = s.padStart(decimals + 1, "0");
  const intPart = padded.slice(0, -decimals).replace(/^0+(?=\d)/, "");
  const fracPart = padded.slice(-decimals).replace(/0+$/, "");
  return fracPart ? `${intPart}.${fracPart}` : intPart;
};

// Adds thousands separators to the integer part of a decimal string produced
// by `formatBaseAmount`, leaving the fractional part untouched.
export const groupThousands = (s: string): string => {
  const dot = s.indexOf(".");
  const intPart = dot === -1 ? s : s.slice(0, dot);
  const rest = dot === -1 ? "" : s.slice(dot);
  return intPart.replace(/\B(?=(\d{3})+(?!\d))/g, ",") + rest;
};

// Parse a normalized decimal string (no thousand separators — store form) back
// to base units for the given `decimals`. Returns 0n for empty/invalid input.
export const parseAmountToBase = (
  amountStr: string,
  decimals: number,
): bigint => {
  if (!amountStr) return 0n;
  const [intRaw, fracRaw] = amountStr.split(".");
  const intPart = intRaw || "0";
  const fracPart = fracRaw || "";
  const padded = (fracPart + "0".repeat(decimals)).slice(0, decimals);
  try {
    return BigInt(intPart) * 10n ** BigInt(decimals) + BigInt(padded || "0");
  } catch {
    return 0n;
  }
};
