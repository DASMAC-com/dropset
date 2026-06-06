//! Decimal floating-point price key for the eCLOB matching engine.
//!
//! `Price` is a `u32` encoding with 8 significant digits and a base-10
//! exponent, designed so that raw unsigned integer comparison matches
//! price-value ordering. See the architecture spec's **Price** section.
//!
//! # Validation
//!
//! `Price` derives `Pod`, so Anchor will deserialize arbitrary bytes
//! into it without validation. [`Price::from_bits`] is similarly
//! unchecked. **Every instruction handler that accepts a `Price` from
//! user input must call [`Price::is_valid`] before the value
//! participates in ordering or arithmetic.** Failing to do so allows
//! invalid bit patterns that mis-sort in the order book.
//!
//! # Truncation
//!
//! [`Price::from_scaled`] normalizes by repeated integer division,
//! which truncates toward zero. This is the conservative choice for a
//! matching engine — it never rounds a price *up* past what was
//! submitted — but callers should be aware that sub-digit precision is
//! silently lost.

use anchor_lang_v2::bytemuck::{Pod, Zeroable};

// ── Constants ────────────────────────────────────────────────────────

/// Bits occupied by the significand (lower portion of the u32).
pub(crate) const SIGNIFICAND_BITS: u32 = 27;

/// Bitmask isolating the 27-bit significand.
pub(crate) const SIGNIFICAND_MASK: u32 = (1u32 << SIGNIFICAND_BITS) - 1;

/// Exponent bias. `unbiased = biased − BIAS`.
pub(crate) const BIAS: u8 = 16;

/// Smallest valid significand (8 significant digits).
pub(crate) const SIGNIFICAND_MIN: u32 = 10_000_000;

/// Largest valid significand (8 significant digits).
pub(crate) const SIGNIFICAND_MAX: u32 = 99_999_999;

/// Largest biased exponent (5 bits, `0..=31`).
pub(crate) const MAX_BIASED_EXPONENT: u8 = 31;

/// Smallest unbiased exponent (`−16`).
pub(crate) const UNBIASED_EXPONENT_MIN: i8 = 0 - BIAS as i8;

/// Largest unbiased exponent (`15`).
pub(crate) const UNBIASED_EXPONENT_MAX: i8 = MAX_BIASED_EXPONENT as i8 - BIAS as i8;

// Compile-time invariants.
const _: () = assert!(SIGNIFICAND_MAX < SIGNIFICAND_MASK);
const _: () = assert!(MAX_BIASED_EXPONENT as u32 == (1u32 << (32 - SIGNIFICAND_BITS)) - 1);

// ── Price ────────────────────────────────────────────────────────────

/// A `u32` decimal floating-point comparison key.
///
/// ```text
///  [5-bit biased exponent][27-bit normalized significand]
///    bits 31..27             bits 26..0
/// ```
///
/// The significand is normalized to exactly 8 significant digits
/// (`10_000_000..=99_999_999`). The exponent is biased by 16 (unbiased
/// range `−16..=15`). The represented value is:
///
/// ```text
/// value = significand × 10^(biased_exponent − 23)
///       = significand × 10^(unbiased_exponent − 7)
/// ```
///
/// Two sentinel encodings: [`Price::ZERO`] (`0x0000_0000`, market sell)
/// and [`Price::INFINITY`] (`0xFFFF_FFFF`, market buy). All other valid
/// bit patterns represent regular prices.
///
/// **Integer order is price order.** With the exponent in the high bits
/// and the significand normalized to a fixed width, unsigned `u32`
/// comparison of two prices matches comparing the values they encode.
/// The encoding is canonical: one bit pattern per representable price.
///
/// **IDL note:** `Price` does not derive `IdlType`. Add the derive if
/// this type appears in an Anchor `#[account]` struct or instruction
/// argument that requires IDL generation.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Pod, Zeroable)]
#[bytemuck(crate = "anchor_lang_v2::bytemuck")]
pub struct Price(u32);

impl core::fmt::Debug for Price {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match () {
            _ if self.is_zero() => write!(f, "Price::ZERO"),
            _ if self.is_infinity() => write!(f, "Price::INFINITY"),
            _ => write!(
                f,
                "Price(0x{:08X}, sig={}, exp={})",
                self.0,
                self.significand(),
                self.unbiased_exponent(),
            ),
        }
    }
}

impl Price {
    /// Market sell sentinel — no minimum fill price.
    pub const ZERO: Self = Self(0);

    /// Market buy sentinel — no maximum fill price.
    pub const INFINITY: Self = Self(u32::MAX);

    /// Construct from a validated significand and **biased** exponent.
    ///
    /// Returns `None` if the significand is outside
    /// `[10_000_000, 99_999_999]` or the biased exponent exceeds 31.
    #[inline]
    pub const fn new(significand: u32, biased_exponent: u8) -> Option<Self> {
        if significand < SIGNIFICAND_MIN
            || significand > SIGNIFICAND_MAX
            || biased_exponent > MAX_BIASED_EXPONENT
        {
            return None;
        }
        let bits = ((biased_exponent as u32) << SIGNIFICAND_BITS) | significand;
        Some(Self(bits))
    }

    /// Construct from a significand and **unbiased** exponent.
    ///
    /// Returns `None` if the significand is outside
    /// `[10_000_000, 99_999_999]` or the exponent is outside `[−16, 15]`.
    #[inline]
    pub const fn encode(significand: u32, unbiased_exponent: i8) -> Option<Self> {
        if unbiased_exponent < UNBIASED_EXPONENT_MIN || unbiased_exponent > UNBIASED_EXPONENT_MAX {
            return None;
        }
        let biased = (unbiased_exponent as i16 + BIAS as i16) as u8;
        Self::new(significand, biased)
    }

    /// Normalize a raw significand and biased exponent into a valid
    /// `Price`. Adjusts the exponent to bring the significand into the
    /// canonical 8-digit range `[10_000_000, 99_999_999]`.
    ///
    /// Digits beyond the 8th are **truncated toward zero** (not
    /// rounded), so the result never exceeds the true input value.
    ///
    /// Returns `None` if the result falls outside the representable
    /// exponent range, or `Price::ZERO` when `sig` is 0.
    pub fn from_scaled(mut sig: u64, mut biased_exp: i16) -> Option<Self> {
        if sig == 0 {
            return Some(Self::ZERO);
        }
        while sig > SIGNIFICAND_MAX as u64 {
            sig /= 10;
            biased_exp += 1;
        }
        while sig < SIGNIFICAND_MIN as u64 {
            sig *= 10;
            biased_exp -= 1;
        }
        if biased_exp < 0 || biased_exp > MAX_BIASED_EXPONENT as i16 {
            return None;
        }
        Self::new(sig as u32, biased_exp as u8)
    }

    /// The raw `u32` bit pattern.
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Wrap a raw `u32` without validation. Used for reading from
    /// trusted on-chain storage where the bits were written by this
    /// program.
    ///
    /// **If the source is untrusted (e.g. instruction arguments or CPI
    /// data), call [`is_valid`](Self::is_valid) before using the
    /// result.** Invalid bit patterns compare incorrectly against valid
    /// prices and will corrupt order-book ordering.
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    pub const fn is_infinity(self) -> bool {
        self.0 == u32::MAX
    }

    /// Whether this is a valid encoding (sentinel or regular price with
    /// significand in `[10_000_000, 99_999_999]`).
    #[inline]
    pub const fn is_valid(self) -> bool {
        if self.0 == 0 || self.0 == u32::MAX {
            return true;
        }
        let sig = self.significand();
        sig >= SIGNIFICAND_MIN && sig <= SIGNIFICAND_MAX
    }

    /// The lower 27 bits. Meaningful only for regular prices.
    #[inline(always)]
    pub const fn significand(self) -> u32 {
        self.0 & SIGNIFICAND_MASK
    }

    /// The upper 5 bits. Meaningful only for regular prices.
    #[inline(always)]
    pub const fn biased_exponent(self) -> u8 {
        (self.0 >> SIGNIFICAND_BITS) as u8
    }

    /// `biased_exponent − BIAS`. Meaningful only for regular prices.
    #[inline(always)]
    pub const fn unbiased_exponent(self) -> i8 {
        self.biased_exponent() as i8 - BIAS as i8
    }

    /// Bid-side heap key: `u32::MAX − self`. Transforms a min-heap
    /// into highest-price-first ordering for bids while keeping the
    /// nonce ascending for price-time priority.
    #[inline(always)]
    pub const fn bid_key(self) -> u32 {
        u32::MAX - self.0
    }

    /// Convert to `f64` for testing.
    #[cfg(test)]
    pub fn to_f64(self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        if self.is_infinity() {
            return f64::INFINITY;
        }
        let sig = self.significand() as f64;
        let shift = self.biased_exponent() as i32 - BIAS as i32 - 7;
        sig * 10f64.powi(shift)
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Sentinels ────────────────────────────────────────────────

    #[test]
    fn zero_sentinel() {
        assert_eq!(Price::ZERO.as_u32(), 0);
        assert!(Price::ZERO.is_zero());
        assert!(!Price::ZERO.is_infinity());
        assert!(Price::ZERO.is_valid());
        assert_eq!(Price::ZERO.to_f64(), 0.0);
    }

    #[test]
    fn infinity_sentinel() {
        assert_eq!(Price::INFINITY.as_u32(), u32::MAX);
        assert!(Price::INFINITY.is_infinity());
        assert!(!Price::INFINITY.is_zero());
        assert!(Price::INFINITY.is_valid());
        assert!(Price::INFINITY.to_f64().is_infinite());
    }

    // ── Construction ─────────────────────────────────────────────

    #[test]
    fn new_valid_boundaries() {
        assert!(Price::new(SIGNIFICAND_MIN, 0).is_some());
        assert!(Price::new(SIGNIFICAND_MAX, MAX_BIASED_EXPONENT).is_some());
        assert!(Price::new(50_000_000, BIAS).is_some());
    }

    #[test]
    fn new_rejects_invalid_significand() {
        assert!(Price::new(SIGNIFICAND_MIN - 1, BIAS).is_none());
        assert!(Price::new(SIGNIFICAND_MAX + 1, BIAS).is_none());
        assert!(Price::new(0, BIAS).is_none());
        assert!(Price::new(u32::MAX, BIAS).is_none());
    }

    #[test]
    fn new_rejects_invalid_exponent() {
        assert!(Price::new(SIGNIFICAND_MIN, MAX_BIASED_EXPONENT + 1).is_none());
    }

    #[test]
    fn encode_valid() {
        let p = Price::encode(12_500_000, 0).unwrap();
        assert_eq!(p.significand(), 12_500_000);
        assert_eq!(p.unbiased_exponent(), 0);
        assert_eq!(p.biased_exponent(), BIAS);
    }

    #[test]
    fn encode_exponent_boundaries() {
        assert!(Price::encode(SIGNIFICAND_MIN, UNBIASED_EXPONENT_MIN).is_some());
        assert!(Price::encode(SIGNIFICAND_MIN, UNBIASED_EXPONENT_MAX).is_some());
        assert!(Price::encode(SIGNIFICAND_MIN, UNBIASED_EXPONENT_MIN - 1).is_none());
        assert!(Price::encode(SIGNIFICAND_MIN, UNBIASED_EXPONENT_MAX + 1).is_none());
    }

    // ── Round-trip ───────────────────────────────────────────────

    #[test]
    fn roundtrip_all_exponents() {
        for &sig in &[SIGNIFICAND_MIN, 12_345_678, 50_000_000, SIGNIFICAND_MAX] {
            for biased_exp in 0..=MAX_BIASED_EXPONENT {
                let p = Price::new(sig, biased_exp).unwrap();
                assert_eq!(p.significand(), sig);
                assert_eq!(p.biased_exponent(), biased_exp);
                assert_eq!(p.unbiased_exponent(), biased_exp as i8 - BIAS as i8,);
                assert!(p.is_valid());
            }
        }
    }

    #[test]
    fn from_bits_roundtrip() {
        let p = Price::encode(98_700_000, 2).unwrap();
        let bits = p.as_u32();
        assert_eq!(Price::from_bits(bits), p);
    }

    // ── Ordering ─────────────────────────────────────────────────

    #[test]
    fn ordering_same_exponent() {
        let p1 = Price::new(10_000_000, BIAS).unwrap();
        let p2 = Price::new(20_000_000, BIAS).unwrap();
        let p3 = Price::new(99_999_999, BIAS).unwrap();
        assert!(p1 < p2);
        assert!(p2 < p3);
    }

    #[test]
    fn ordering_across_exponents() {
        // Higher exponent always beats higher significand at lower exp.
        let big_sig_low_exp = Price::new(SIGNIFICAND_MAX, 10).unwrap();
        let small_sig_high_exp = Price::new(SIGNIFICAND_MIN, 11).unwrap();
        assert!(big_sig_low_exp < small_sig_high_exp);
    }

    #[test]
    fn ordering_matches_value() {
        let one = Price::encode(10_000_000, 0).unwrap();
        let two = Price::encode(20_000_000, 0).unwrap();
        assert!(one < two);
        assert!(one.to_f64() < two.to_f64());

        // 9.9999999 vs 10.000000 — crosses an exponent boundary.
        let just_under_10 = Price::encode(99_999_999, 0).unwrap();
        let ten = Price::encode(10_000_000, 1).unwrap();
        assert!(just_under_10 < ten);
        assert!(just_under_10.to_f64() < ten.to_f64());
    }

    #[test]
    fn ordering_adjacent_exponents_exhaustive() {
        // For each adjacent exponent pair, max price at e is less than
        // min price at e+1.
        for biased_exp in 0..MAX_BIASED_EXPONENT {
            let max_at_e = Price::new(SIGNIFICAND_MAX, biased_exp).unwrap();
            let min_at_next = Price::new(SIGNIFICAND_MIN, biased_exp + 1).unwrap();
            assert!(
                max_at_e < min_at_next,
                "max at exp {} should be < min at exp {}",
                biased_exp,
                biased_exp + 1,
            );
            assert!(max_at_e.to_f64() < min_at_next.to_f64());
        }
    }

    #[test]
    fn sentinels_bracket_all_regular_prices() {
        let min_regular = Price::new(SIGNIFICAND_MIN, 0).unwrap();
        let max_regular = Price::new(SIGNIFICAND_MAX, MAX_BIASED_EXPONENT).unwrap();
        assert!(Price::ZERO < min_regular);
        assert!(max_regular < Price::INFINITY);
    }

    #[test]
    fn canonical_encoding() {
        // Adjacent representable values encode to distinct bit patterns.
        let a = Price::encode(10_000_000, 1).unwrap(); // 10.000000
        let b = Price::encode(99_999_999, 0).unwrap(); // 9.9999999
        assert_ne!(a, b);
        assert!(a > b);
    }

    // ── Bid key ──────────────────────────────────────────────────

    #[test]
    fn bid_key_inverts_ordering() {
        let low = Price::encode(10_000_000, 0).unwrap();
        let high = Price::encode(50_000_000, 0).unwrap();
        // Higher price → lower bid_key (min-heap pops best bid first).
        assert!(high.bid_key() < low.bid_key());
    }

    #[test]
    fn bid_key_sentinels() {
        assert_eq!(Price::ZERO.bid_key(), u32::MAX);
        assert_eq!(Price::INFINITY.bid_key(), 0);
    }

    // ── from_scaled ──────────────────────────────────────────────

    #[test]
    fn from_scaled_identity() {
        let p = Price::from_scaled(50_000_000, BIAS as i16).unwrap();
        assert_eq!(p.significand(), 50_000_000);
        assert_eq!(p.biased_exponent(), BIAS);
    }

    #[test]
    fn from_scaled_downscale() {
        // 100_000_000 → /10 → 10_000_000, exp +1.
        let p = Price::from_scaled(100_000_000, 10).unwrap();
        assert_eq!(p.significand(), 10_000_000);
        assert_eq!(p.biased_exponent(), 11);
    }

    #[test]
    fn from_scaled_upscale() {
        // 5_000_000 → *10 → 50_000_000, exp −1.
        let p = Price::from_scaled(5_000_000, 10).unwrap();
        assert_eq!(p.significand(), 50_000_000);
        assert_eq!(p.biased_exponent(), 9);
    }

    #[test]
    fn from_scaled_large_downscale() {
        // 999_999_990_000 → four /10 steps → 99_999_999, exp +4.
        let p = Price::from_scaled(999_999_990_000, 5).unwrap();
        assert_eq!(p.significand(), 99_999_999);
        assert_eq!(p.biased_exponent(), 9);
    }

    #[test]
    fn from_scaled_truncation() {
        // 123_456_789 → /10 → 12_345_678 (9 truncated), exp +1.
        let p = Price::from_scaled(123_456_789, 10).unwrap();
        assert_eq!(p.significand(), 12_345_678);
        assert_eq!(p.biased_exponent(), 11);
    }

    #[test]
    fn from_scaled_zero_input() {
        assert_eq!(Price::from_scaled(0, 10), Some(Price::ZERO));
    }

    #[test]
    fn from_scaled_exponent_overflow() {
        // After downscale, exponent would exceed 31.
        assert!(Price::from_scaled(100_000_000, MAX_BIASED_EXPONENT as i16).is_none());
    }

    #[test]
    fn from_scaled_exponent_underflow() {
        // sig=1 needs 7 upscale steps → biased_exp = 5 − 7 = −2.
        assert!(Price::from_scaled(1, 5).is_none());
    }

    #[test]
    fn from_scaled_u64_max() {
        // u64::MAX ≈ 1.8e19 → ~12 division steps, biased_exp += 12.
        // Starting at biased_exp=0, result should be biased_exp=12.
        let p = Price::from_scaled(u64::MAX, 0).unwrap();
        assert_eq!(p.significand(), 18_446_744);
        assert_eq!(p.biased_exponent(), 12);
        assert!(p.is_valid());

        // Starting at biased_exp=20, 12 divisions push to 32 > 31.
        assert!(Price::from_scaled(u64::MAX, 20).is_none());
    }

    #[test]
    fn from_scaled_ppm_ask_offset() {
        // Simulate flush ask offset: price × (PPM + offset) / PPM.
        let ref_sig: u64 = 10_850_000; // EUR/USD 1.0850
        let ref_exp: i16 = BIAS as i16; // biased exp for unbiased 0
        let offset: u64 = 500; // 0.05% = 500 ppm

        let ppm: u64 = 1_000_000;
        let new_sig = ref_sig * (ppm + offset) / ppm;
        let p = Price::from_scaled(new_sig, ref_exp).unwrap();

        // 10_850_000 × 1_000_500 / 1_000_000 = 10_855_425
        assert_eq!(p.significand(), 10_855_425);
        assert_eq!(p.biased_exponent(), BIAS);
    }

    #[test]
    fn from_scaled_ppm_bid_offset() {
        let ref_sig: u64 = 10_850_000;
        let ref_exp: i16 = BIAS as i16;
        let offset: u64 = 500;

        let ppm: u64 = 1_000_000;
        let factor = ppm.saturating_sub(offset);
        let new_sig = ref_sig * factor / ppm;
        let p = Price::from_scaled(new_sig, ref_exp).unwrap();

        // 10_850_000 × 999_500 / 1_000_000 = 10_844_575
        assert_eq!(p.significand(), 10_844_575);
        assert_eq!(p.biased_exponent(), BIAS);
    }

    #[test]
    fn from_scaled_ppm_large_offset_wraps_exponent() {
        // Large offset (100% = doubling) that pushes sig above 100M.
        let ref_sig: u64 = 50_000_000;
        let ref_exp: i16 = BIAS as i16;
        let offset: u64 = 1_000_000; // 100%

        let ppm: u64 = 1_000_000;
        let new_sig = ref_sig * (ppm + offset) / ppm; // 100_000_000
        let p = Price::from_scaled(new_sig, ref_exp).unwrap();

        assert_eq!(p.significand(), 10_000_000);
        assert_eq!(p.biased_exponent(), BIAS + 1);
    }

    #[test]
    fn from_scaled_bid_offset_saturates_to_zero() {
        // offset >= PPM → factor = 0 → sig = 0 → Price::ZERO.
        let ref_sig: u64 = 50_000_000;
        let offset: u64 = 1_000_000;

        let ppm: u64 = 1_000_000;
        let factor = ppm.saturating_sub(offset);
        let new_sig = ref_sig * factor / ppm;
        assert_eq!(new_sig, 0);
        assert_eq!(Price::from_scaled(new_sig, BIAS as i16), Some(Price::ZERO));
    }

    // ── Specific examples ────────────────────────────────────────

    #[test]
    fn example_price_987() {
        // 987 = 9.87 × 10^2 → sig = 98_700_000, unbiased_exp = 2.
        let p = Price::encode(98_700_000, 2).unwrap();
        let v = p.to_f64();
        assert!((v - 987.0).abs() < 1e-10);
        assert_eq!(p.significand(), 98_700_000);
        assert_eq!(p.unbiased_exponent(), 2);
        assert_eq!(p.biased_exponent(), 18);
    }

    #[test]
    fn fx_price_1_0850() {
        // EUR/USD ≈ 1.0850 → sig = 10_850_000, unbiased_exp = 0.
        let p = Price::encode(10_850_000, 0).unwrap();
        assert!((p.to_f64() - 1.085).abs() < 1e-10);
    }

    #[test]
    fn fx_price_exact_decimal() {
        // 1.0850 encodes without binary rounding because base-10
        // exponents keep decimal prices exact.
        let p = Price::encode(10_850_000, 0).unwrap();
        // Re-derive: sig × 10^(unbiased − 7) = 10_850_000 × 10^(−7)
        // = 1.0850000 exactly.
        assert_eq!(p.significand(), 10_850_000);
        assert_eq!(p.unbiased_exponent(), 0);
    }

    #[test]
    fn min_representable_price() {
        // sig = 10_000_000, unbiased = −16 → 1.0 × 10^−16.
        let p = Price::encode(SIGNIFICAND_MIN, UNBIASED_EXPONENT_MIN).unwrap();
        let v = p.to_f64();
        assert!((v - 1e-16).abs() / 1e-16 < 1e-7);
    }

    #[test]
    fn max_representable_price() {
        // sig = 99_999_999, unbiased = 15 → ~9.9999999 × 10^15.
        let p = Price::encode(SIGNIFICAND_MAX, UNBIASED_EXPONENT_MAX).unwrap();
        let v = p.to_f64();
        let expected = 9.9999999e15;
        assert!((v - expected).abs() / expected < 1e-7);
    }

    // ── Validity ─────────────────────────────────────────────────

    #[test]
    fn is_valid_rejects_bad_significand() {
        // Manually construct an invalid price (significand too low).
        let bad = Price(5_000_000 | ((BIAS as u32) << SIGNIFICAND_BITS));
        assert!(!bad.is_valid());

        // Significand too high (but not u32::MAX).
        let bad2 = Price(SIGNIFICAND_MASK | ((10u32) << SIGNIFICAND_BITS));
        assert!(!bad2.is_valid());
    }

    #[test]
    fn is_valid_accepts_all_constructed_prices() {
        for &sig in &[SIGNIFICAND_MIN, 55_555_555, SIGNIFICAND_MAX] {
            for exp in [0, BIAS, MAX_BIASED_EXPONENT] {
                assert!(Price::new(sig, exp).unwrap().is_valid());
            }
        }
    }

    // ── Debug format ─────────────────────────────────────────────

    #[test]
    fn debug_format() {
        let z = format!("{:?}", Price::ZERO);
        assert_eq!(z, "Price::ZERO");

        let inf = format!("{:?}", Price::INFINITY);
        assert_eq!(inf, "Price::INFINITY");

        let p = Price::encode(12_345_678, 3).unwrap();
        let s = format!("{:?}", p);
        assert!(s.contains("sig=12345678"));
        assert!(s.contains("exp=3"));
    }

    // ── Zeroable ─────────────────────────────────────────────────

    #[test]
    fn zeroable_yields_price_zero() {
        let z: Price = Price::zeroed();
        assert_eq!(z, Price::ZERO);
        assert!(z.is_zero());
    }
}
