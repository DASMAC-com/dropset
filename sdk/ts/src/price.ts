/**
 * Human-facing `Price` codec.
 *
 * The on-chain `Price` is a `u32` decimal floating-point comparison key
 * (see programs/dropset/src/price.rs and architecture.md § Price). The
 * IDL exposes it only as raw `u32` bits — the generated `Price` type is
 * `number` (those bits) — so this module layers the bits <-> decimal
 * conversion the frontend needs to display prices and to build
 * `set_reference_price` / `swap` arguments.
 *
 * Layout: `[5-bit biased exponent][27-bit normalized significand]`.
 * The represented value is `significand * 10^(unbiasedExponent - 7)`,
 * with the significand normalized to 8 significant digits
 * (`10_000_000..=99_999_999`) and the exponent biased by 16.
 */

const SIGNIFICAND_BITS = 27;
const SIGNIFICAND_MASK = (1 << SIGNIFICAND_BITS) - 1;
const BIAS = 16;
const SIGNIFICAND_MIN = 10_000_000;
const SIGNIFICAND_MAX = 99_999_999;
const MAX_BIASED_EXPONENT = 31;

/** Market-sell sentinel — no minimum fill price. Raw bits `0x00000000`. */
export const PRICE_ZERO = 0;
/** Market-buy sentinel — no maximum fill price. Raw bits `0xFFFFFFFF`. */
export const PRICE_INFINITY = 0xffff_ffff;

/** Raw `Price` bits, as carried in instruction args, accounts, and events. */
export type PriceBits = number;

export function isZeroPrice(bits: PriceBits): boolean {
  return bits >>> 0 === PRICE_ZERO;
}

export function isInfinityPrice(bits: PriceBits): boolean {
  return bits >>> 0 === PRICE_INFINITY;
}

/** The lower 27 bits. Meaningful only for regular (non-sentinel) prices. */
export function priceSignificand(bits: PriceBits): number {
  return (bits >>> 0) & SIGNIFICAND_MASK;
}

/** The upper 5 bits. Meaningful only for regular prices. */
export function priceBiasedExponent(bits: PriceBits): number {
  return (bits >>> 0) >>> SIGNIFICAND_BITS;
}

/** `biasedExponent - 16`. Meaningful only for regular prices. */
export function priceUnbiasedExponent(bits: PriceBits): number {
  return priceBiasedExponent(bits) - BIAS;
}

/**
 * Whether `bits` is a valid encoding: a sentinel, or a regular price
 * whose significand sits in `[10_000_000, 99_999_999]`. Mirrors
 * `Price::is_valid` — invalid patterns mis-sort in the order book, so
 * callers must check user-supplied bits before submitting them.
 */
export function isValidPrice(bits: PriceBits): boolean {
  const u = bits >>> 0;
  if (u === PRICE_ZERO || u === PRICE_INFINITY) return true;
  const sig = priceSignificand(u);
  return sig >= SIGNIFICAND_MIN && sig <= SIGNIFICAND_MAX;
}

/** Assemble raw bits from a validated significand and **biased** exponent. */
export function priceFromParts(
  significand: number,
  biasedExponent: number,
): PriceBits {
  if (
    significand < SIGNIFICAND_MIN ||
    significand > SIGNIFICAND_MAX ||
    biasedExponent < 0 ||
    biasedExponent > MAX_BIASED_EXPONENT
  ) {
    throw new RangeError(
      `Price out of range: significand=${significand}, biasedExponent=${biasedExponent}`,
    );
  }
  return ((biasedExponent << SIGNIFICAND_BITS) | significand) >>> 0;
}

/**
 * Normalize a scaled integer significand + biased exponent into canonical
 * bits, mirroring `Price::from_scaled`. Digits beyond the 8th are
 * truncated toward zero. Throws if the result falls outside the
 * representable exponent range.
 */
function fromScaled(sig: number, biasedExp: number): PriceBits {
  if (sig === 0) return PRICE_ZERO;
  let s = Math.trunc(sig);
  let e = biasedExp;
  while (s > SIGNIFICAND_MAX) {
    s = Math.trunc(s / 10);
    e += 1;
  }
  while (s < SIGNIFICAND_MIN) {
    s *= 10;
    e -= 1;
  }
  if (e < 0 || e > MAX_BIASED_EXPONENT) {
    throw new RangeError(`Price exponent out of range: biasedExponent=${e}`);
  }
  return priceFromParts(s, e);
}

/**
 * Encode a decimal price (e.g. `1.085`) into raw `Price` bits.
 *
 * Intended for the FX value range the protocol targets (roughly
 * `1e-6 .. 1e6`), where `value * 1e7` stays within JS safe-integer
 * precision. Values are truncated to 8 significant digits. For the
 * no-bound sentinels pass {@link PRICE_ZERO} / {@link PRICE_INFINITY}
 * directly.
 */
export function encodePrice(value: number): PriceBits {
  if (!Number.isFinite(value) || value < 0) {
    throw new RangeError(`Price must be a finite, non-negative number: ${value}`);
  }
  if (value === 0) return PRICE_ZERO;
  // value = significand * 10^(unbiased - 7); choose unbiased = 0
  // (biased = BIAS) so the scaled integer is value * 1e7, then let
  // fromScaled renormalize the significand into the canonical range.
  return fromScaled(Math.trunc(value * 1e7), BIAS);
}

/** Decode raw `Price` bits to a JS number. Sentinels map to `0` / `Infinity`. */
export function decodePrice(bits: PriceBits): number {
  const u = bits >>> 0;
  if (u === PRICE_ZERO) return 0;
  if (u === PRICE_INFINITY) return Number.POSITIVE_INFINITY;
  return priceSignificand(u) * 10 ** (priceUnbiasedExponent(u) - 7);
}
