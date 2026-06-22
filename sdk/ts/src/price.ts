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

const SIGNIFICAND_MIN_BIG = BigInt(SIGNIFICAND_MIN);
const SIGNIFICAND_MAX_BIG = BigInt(SIGNIFICAND_MAX);

/**
 * Normalize a scaled integer significand + biased exponent into canonical
 * bits, mirroring `Price::from_scaled`. The significand is a `bigint` so
 * the normalization is exact integer math — matching Rust's `u64`
 * division bit-for-bit even past JS's 2^53 safe-integer range (callers
 * lift values that can exceed it; see {@link weightedAverage}). Digits
 * beyond the 8th are truncated toward zero. Throws if the result falls
 * outside the representable exponent range.
 */
function fromScaled(sig: bigint, biasedExp: number): PriceBits {
  if (sig === 0n) return PRICE_ZERO;
  let s = sig;
  let e = biasedExp;
  while (s > SIGNIFICAND_MAX_BIG) {
    s /= 10n;
    e += 1;
  }
  while (s < SIGNIFICAND_MIN_BIG) {
    s *= 10n;
    e -= 1;
  }
  if (e < 0 || e > MAX_BIASED_EXPONENT) {
    throw new RangeError(`Price exponent out of range: biasedExponent=${e}`);
  }
  // `s` is now in `[10_000_000, 99_999_999]`, well within safe-integer range.
  return priceFromParts(Number(s), e);
}

/**
 * Encode a decimal price (e.g. `1.085`) into raw `Price` bits.
 *
 * Intended for the FX value range the protocol targets (roughly
 * `1e-6 .. 1e6`); values are truncated to 8 significant digits. Throws a
 * `RangeError` on non-finite/negative input, and on a value so large that
 * `value * 1e7` would not fit in a `u64` — the boundary at which the Rust
 * `from_value` cast would otherwise silently saturate, so both forks
 * reject the same inputs. For the no-bound sentinels pass
 * {@link PRICE_ZERO} / {@link PRICE_INFINITY} directly.
 */
export function encodePrice(value: number): PriceBits {
  if (!Number.isFinite(value) || value < 0) {
    throw new RangeError(`Price must be a finite, non-negative number: ${value}`);
  }
  if (value === 0) return PRICE_ZERO;
  const scaled = value * 1e7;
  // Mirror Rust `from_value`: reject what would overflow the `u64` scaling
  // intermediate. `2 ** 64` is `u64::MAX as f64` (which rounds up to 2^64),
  // so any `scaled` below it truncates losslessly toward zero into range.
  if (scaled >= 2 ** 64) {
    throw new RangeError(`Price out of range: ${value}`);
  }
  // value = significand * 10^(unbiased - 7); choose unbiased = 0
  // (biased = BIAS) so the scaled integer is value * 1e7, then let
  // fromScaled renormalize the significand into the canonical range.
  return fromScaled(BigInt(Math.trunc(scaled)), BIAS);
}

/** Decode raw `Price` bits to a JS number. Sentinels map to `0` / `Infinity`. */
export function decodePrice(bits: PriceBits): number {
  const u = bits >>> 0;
  if (u === PRICE_ZERO) return 0;
  if (u === PRICE_INFINITY) return Number.POSITIVE_INFINITY;
  return priceSignificand(u) * 10 ** (priceUnbiasedExponent(u) - 7);
}

/** `quote_for_base(INFINITY)` / `base_for_quote(ZERO)` sentinel value. */
const U128_MAX = (1n << 128n) - 1n;
const U64_MAX = (1n << 64n) - 1n;

/** Clamp to `u128::MAX`, mirroring Rust's `saturating_*` on `u128`. */
const satU128 = (x: bigint): bigint => (x > U128_MAX ? U128_MAX : x);

/**
 * `base * price`, rounded toward zero — the exact integer math the on-chain
 * matcher uses (mirrors Rust `Price::quote_for_base`). `ZERO -> 0n`,
 * `INFINITY -> U128_MAX`. Use this (not {@link decodePrice} float math) for
 * cross-language-consistent ratios. `bigint` carries the full range; values
 * are not saturated to `u64` (lossless for the FX atom scales).
 */
export function quoteForBase(bits: PriceBits, base: bigint): bigint {
  const u = bits >>> 0;
  if (u === PRICE_ZERO) return 0n;
  if (u === PRICE_INFINITY) return U128_MAX;
  const sig = BigInt(priceSignificand(u));
  const unb = priceUnbiasedExponent(u) - 7;
  let x = base * sig;
  if (unb >= 0) for (let i = 0; i < unb; i++) x *= 10n;
  else for (let i = 0; i < -unb; i++) x /= 10n;
  return x;
}

/**
 * `quote / price`, rounded toward zero (mirrors Rust `Price::base_for_quote`).
 * `ZERO -> U128_MAX`, `INFINITY -> 0n`.
 */
export function baseForQuote(bits: PriceBits, quote: bigint): bigint {
  const u = bits >>> 0;
  if (u === PRICE_ZERO) return U128_MAX;
  if (u === PRICE_INFINITY) return 0n;
  const sig = BigInt(priceSignificand(u));
  const unb = priceUnbiasedExponent(u) - 7;
  let num = quote;
  let den = sig;
  if (unb >= 0) for (let i = 0; i < unb; i++) den *= 10n;
  else for (let i = 0; i < -unb; i++) num *= 10n;
  return den === 0n ? 0n : num / den;
}

/**
 * Shares-weighted average of two prices in *decoded-value* space (mirrors
 * Rust `Price::weighted_average`). Both prices are lifted to `value × 10^9`,
 * averaged by the integer weights, then re-encoded via {@link fromScaled}
 * (truncating toward zero — one ULP per merge, acceptable for cost-basis
 * bookkeeping). Backs the depositor cost-basis merge on a top-off deposit
 * ({@link mergeEntryBasis}).
 *
 * Returns `self` when both weights are zero; {@link PRICE_ZERO} if either
 * input is the ZERO sentinel; {@link PRICE_INFINITY} if either is INFINITY
 * — the engine's conservative sentinel collapse. Falls back to
 * {@link PRICE_ZERO} if the re-encode overflows the exponent range (Rust's
 * `unwrap_or(ZERO)`).
 */
export function weightedAverage(
  self: PriceBits,
  other: PriceBits,
  wSelf: bigint,
  wOther: bigint,
): PriceBits {
  if (wSelf === 0n && wOther === 0n) return self >>> 0;
  if (wSelf === 0n) return other >>> 0;
  if (wOther === 0n) return self >>> 0;
  if (isZeroPrice(self) || isZeroPrice(other)) return PRICE_ZERO;
  if (isInfinityPrice(self) || isInfinityPrice(other)) return PRICE_INFINITY;
  const SCALE = 1_000_000_000n;
  const vSelf = quoteForBase(self, SCALE);
  const vOther = quoteForBase(other, SCALE);
  // Mirror Rust's `u128` `saturating_mul` / `saturating_add`: clamp each
  // weighted product, their sum, and the weight total at `u128::MAX`. For
  // FX-range prices nothing saturates and this is a no-op, but for
  // structurally-valid prices above that band the products can exceed
  // `u128`, where plain bigint would otherwise diverge from the engine.
  const total = satU128(wSelf + wOther);
  const avg = satU128(satU128(wSelf * vSelf) + satU128(wOther * vOther)) / total;
  // value × 10^9 = sig × 10^(biased − 14) ⟹ the scaled input's biased
  // exponent is 14. Drive `fromScaled` from the `bigint` directly: `avg`
  // can exceed 2^53 (price value > ~9e6), where a `Number(avg)` round
  // would flip the 8th significand digit vs Rust's exact `u64` `from_scaled`.
  const avgU64 = avg > U64_MAX ? U64_MAX : avg;
  try {
    return fromScaled(avgU64, 14);
  } catch {
    return PRICE_ZERO;
  }
}
