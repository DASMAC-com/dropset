//! `Price` codec — a solana-free port of the program's
//! `programs/dropset/src/price.rs`.
//!
//! `Price` is a `u32` decimal floating-point comparison key with 8
//! significant digits and a base-10 exponent, designed so raw unsigned
//! integer comparison matches price-value ordering. The IDL exposes it
//! only as raw `u32` bits (the on-chain type does not derive `IdlType`),
//! so the matching simulator and any off-chain renderer need this
//! decode/encode + the `quote_for_base` / `base_for_quote` ratio math.
//!
//! This is a hand-mirror of the on-chain logic. Per interface.md § SDK
//! (spine B), the long-term home for this math is a shared `price-core`
//! crate compiled to WASM with conformance vectors run in both Rust and
//! TS CI; until that lands, keep this in lockstep with the program copy.

const SIGNIFICAND_BITS: u32 = 27;
const SIGNIFICAND_MASK: u32 = (1 << SIGNIFICAND_BITS) - 1;
const BIAS: u8 = 16;
const SIGNIFICAND_MIN: u32 = 10_000_000;
const SIGNIFICAND_MAX: u32 = 99_999_999;
const MAX_BIASED_EXPONENT: u8 = 31;

/// A `u32` decimal floating-point comparison key. See the module docs and
/// architecture.md § Price.
#[repr(transparent)]
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
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
    /// Market-sell sentinel — no minimum fill price.
    pub const ZERO: Self = Self(0);
    /// Market-buy sentinel — no maximum fill price.
    pub const INFINITY: Self = Self(u32::MAX);

    /// Wrap raw `u32` bits without validation. Call [`is_valid`](Self::is_valid)
    /// before use when the source is untrusted.
    #[inline(always)]
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// The raw `u32` bit pattern.
    #[inline(always)]
    pub const fn as_u32(self) -> u32 {
        self.0
    }

    /// Encode a decimal value (e.g. `1.085`) into a `Price`. Intended for
    /// the FX value range (`value * 1e7` within f64 integer precision);
    /// truncates to 8 significant digits. `0.0` maps to [`Price::ZERO`].
    /// Returns `None` for non-finite/negative input or out-of-range
    /// exponent. Mirrors the TS `encodePrice`.
    pub fn from_value(value: f64) -> Option<Self> {
        if !value.is_finite() || value < 0.0 {
            return None;
        }
        if value == 0.0 {
            return Some(Self::ZERO);
        }
        Self::from_scaled((value * 1e7) as u64, BIAS as i16)
    }

    /// Construct from a validated significand and **biased** exponent.
    #[inline]
    pub const fn new(significand: u32, biased_exponent: u8) -> Option<Self> {
        if significand < SIGNIFICAND_MIN
            || significand > SIGNIFICAND_MAX
            || biased_exponent > MAX_BIASED_EXPONENT
        {
            return None;
        }
        Some(Self(
            ((biased_exponent as u32) << SIGNIFICAND_BITS) | significand,
        ))
    }

    /// Construct from a significand and **unbiased** exponent (`[-16, 15]`).
    #[inline]
    pub const fn encode(significand: u32, unbiased_exponent: i8) -> Option<Self> {
        let min = 0 - BIAS as i8;
        let max = MAX_BIASED_EXPONENT as i8 - BIAS as i8;
        if unbiased_exponent < min || unbiased_exponent > max {
            return None;
        }
        Self::new(significand, (unbiased_exponent as i16 + BIAS as i16) as u8)
    }

    /// Normalize a scaled significand + biased exponent into canonical
    /// bits. Truncates toward zero; returns `None` on exponent overflow.
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

    #[inline(always)]
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    #[inline(always)]
    pub const fn is_infinity(self) -> bool {
        self.0 == u32::MAX
    }

    /// Whether this is a valid encoding (sentinel or in-range significand).
    #[inline]
    pub const fn is_valid(self) -> bool {
        if self.0 == 0 || self.0 == u32::MAX {
            return true;
        }
        let sig = self.significand();
        sig >= SIGNIFICAND_MIN && sig <= SIGNIFICAND_MAX
    }

    #[inline(always)]
    pub const fn significand(self) -> u32 {
        self.0 & SIGNIFICAND_MASK
    }

    #[inline(always)]
    pub const fn biased_exponent(self) -> u8 {
        (self.0 >> SIGNIFICAND_BITS) as u8
    }

    #[inline(always)]
    pub const fn unbiased_exponent(self) -> i8 {
        self.biased_exponent() as i8 - BIAS as i8
    }

    /// Bid-side heap key: `u32::MAX - self`, so a min-key sort yields
    /// highest-price-first for bids.
    #[inline(always)]
    pub const fn bid_key(self) -> u32 {
        u32::MAX - self.0
    }

    /// `base * price`, rounded toward zero. `ZERO -> 0`, `INFINITY -> u128::MAX`.
    #[inline]
    pub fn quote_for_base(self, base: u64) -> u128 {
        if self.is_zero() {
            return 0;
        }
        if self.is_infinity() {
            return u128::MAX;
        }
        let sig = self.significand() as u128;
        let unb = self.unbiased_exponent() as i32 - 7;
        let mut x = (base as u128).saturating_mul(sig);
        if unb >= 0 {
            for _ in 0..unb {
                x = x.saturating_mul(10);
            }
        } else {
            for _ in 0..(-unb) {
                x /= 10;
            }
        }
        x
    }

    /// `quote / price`, rounded toward zero. `ZERO -> u128::MAX`, `INFINITY -> 0`.
    #[inline]
    pub fn base_for_quote(self, quote: u64) -> u128 {
        if self.is_zero() {
            return u128::MAX;
        }
        if self.is_infinity() {
            return 0;
        }
        let sig = self.significand() as u128;
        let unb = self.unbiased_exponent() as i32 - 7;
        let mut num = quote as u128;
        let mut den = sig;
        if unb >= 0 {
            for _ in 0..unb {
                den = den.saturating_mul(10);
            }
        } else {
            for _ in 0..(-unb) {
                num = num.saturating_mul(10);
            }
        }
        if den == 0 {
            return 0;
        }
        num / den
    }

    /// Decode to `f64`. Display/diagnostics only — never used in fill math.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sentinels() {
        assert!(Price::ZERO.is_zero() && Price::ZERO.is_valid());
        assert!(Price::INFINITY.is_infinity() && Price::INFINITY.is_valid());
        assert_eq!(Price::ZERO.bid_key(), u32::MAX);
        assert_eq!(Price::INFINITY.bid_key(), 0);
    }

    #[test]
    fn fx_ratio_math() {
        // EUR/USD 1.0850
        let p = Price::encode(10_850_000, 0).unwrap();
        assert_eq!(p.quote_for_base(1_000_000), 1_085_000);
        assert_eq!(p.base_for_quote(1_085_000), 1_000_000);
        assert!((p.to_f64() - 1.085).abs() < 1e-9);
    }

    #[test]
    fn integer_order_is_price_order() {
        let a = Price::encode(99_999_999, 0).unwrap(); // 9.9999999
        let b = Price::encode(10_000_000, 1).unwrap(); // 10.0
        assert!(a < b && a.as_u32() < b.as_u32());
    }

    #[test]
    fn from_scaled_truncates() {
        let p = Price::from_scaled(123_456_789, 10).unwrap();
        assert_eq!(p.significand(), 12_345_678);
        assert_eq!(p.biased_exponent(), 11);
    }
}
