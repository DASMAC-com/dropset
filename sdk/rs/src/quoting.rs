//! Native-vs-relative quoting.
//!
//! The program quotes *relatively*: a single `reference_price` plus a
//! [`LiquidityProfile`](crate::layout::LiquidityProfile) of per-level ppm
//! offsets and bps sizes (architecture.md § LiquidityProfile). This module
//! adds the **native CLOB** direction — a leader (or MM
//! bot) specifies a full book of absolute price levels and atom sizes, and
//! [`NativeBook::to_profile`] translates it into the relative profile the
//! program stores, anchored to a chosen reference price.
//!
//! The translation is the inverse of the on-chain flush
//! (`swap::flush_level_price`): an ask at absolute price `P` against
//! reference `R` becomes a ppm offset `(P/R - 1)·1e6`; a level of `size`
//! atoms against an inventory leg of `leg` atoms becomes `size/leg·10000`
//! bps. Sizes are bounded by the per-side `Σ size_bps ≤ 10000` invariant
//! the program enforces at `set_liquidity_profile`.

use solana_instruction::Instruction;
use solana_pubkey::Pubkey;

use crate::generated::instructions::{
    SetLiquidityProfile, SetLiquidityProfileInstructionArgs, SetReferencePrice,
    SetReferencePriceInstructionArgs,
};
use crate::layout::{Level, LiquidityProfile, BPS, N_LEVELS, PPM};
use crate::price::Price;

/// Common scale for decoding a `Price` to an integer value before taking
/// ratios — `value × 10^9`, matching `Price::weighted_average`.
const SCALE: u64 = 1_000_000_000;

/// One level of a native (absolute-price) book.
#[derive(Clone, Copy, Debug)]
pub struct NativeLevel {
    /// Absolute price for this level.
    pub price: Price,
    /// Allowance in atoms: **base** atoms for asks, **quote** atoms for
    /// bids (matching the on-chain materialized `Position.size`).
    pub size: u64,
    /// Per-level expiry, in slots after the reference's `quote_slot`.
    pub expiry_offset: u32,
}

/// A native book: absolute-price bid/ask ladders, top-of-book first. At
/// most [`N_LEVELS`] per side.
#[derive(Clone, Debug, Default)]
pub struct NativeBook {
    pub bids: Vec<NativeLevel>,
    pub asks: Vec<NativeLevel>,
}

/// Errors translating a native book into a relative profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotingError {
    /// More than [`N_LEVELS`] levels on a side.
    TooManyLevels,
    /// Reference price is a sentinel / zero — no ratio is defined.
    InvalidReference,
    /// An ask priced at or below the reference (offsets are unsigned;
    /// asks sit above, bids below).
    AskBelowReference,
    /// A bid priced at or above the reference.
    BidAboveReference,
    /// A level (or per-side total) exceeds the inventory leg
    /// (`size_bps > 10000`), or the inventory leg is zero.
    SizeExceedsInventory,
    /// The ppm offset overflowed `Ppm32` (u32).
    OffsetOverflow,
}

fn level_to_relative(
    lvl: &NativeLevel,
    leg_atoms: u64,
    reference: Price,
    is_ask: bool,
) -> Result<Level, QuotingError> {
    let ref_val = reference.quote_for_base(SCALE);
    if ref_val == 0 || reference.is_infinity() {
        return Err(QuotingError::InvalidReference);
    }
    let p_val = lvl.price.quote_for_base(SCALE);
    let ratio_ppm = p_val.saturating_mul(PPM as u128) / ref_val;
    let offset = if is_ask {
        if ratio_ppm < PPM as u128 {
            return Err(QuotingError::AskBelowReference);
        }
        ratio_ppm - PPM as u128
    } else {
        if ratio_ppm > PPM as u128 {
            return Err(QuotingError::BidAboveReference);
        }
        PPM as u128 - ratio_ppm
    };
    if offset > u32::MAX as u128 {
        return Err(QuotingError::OffsetOverflow);
    }
    if leg_atoms == 0 {
        return Err(QuotingError::SizeExceedsInventory);
    }
    let size_bps = lvl.size as u128 * BPS as u128 / leg_atoms as u128;
    if size_bps > BPS as u128 {
        return Err(QuotingError::SizeExceedsInventory);
    }
    Ok(Level {
        price_offset: (offset as u32).into(),
        size_bps: (size_bps as u16).into(),
        expiry_offset: lvl.expiry_offset.into(),
    })
}

impl NativeBook {
    /// Translate this native book into a relative [`LiquidityProfile`],
    /// anchored to `reference` and sized against the vault's current
    /// `(base_atoms, quote_atoms)`. Ask sizes are fractions of
    /// `base_atoms`, bid sizes fractions of `quote_atoms`.
    pub fn to_profile(
        &self,
        reference: Price,
        base_atoms: u64,
        quote_atoms: u64,
    ) -> Result<LiquidityProfile, QuotingError> {
        use bytemuck::Zeroable;
        if self.bids.len() > N_LEVELS || self.asks.len() > N_LEVELS {
            return Err(QuotingError::TooManyLevels);
        }
        let mut profile = LiquidityProfile::zeroed();
        for (i, lvl) in self.asks.iter().enumerate() {
            profile.asks[i] = level_to_relative(lvl, base_atoms, reference, true)?;
        }
        for (i, lvl) in self.bids.iter().enumerate() {
            profile.bids[i] = level_to_relative(lvl, quote_atoms, reference, false)?;
        }
        // Canonical per-side `Σ size_bps ≤ BPS` gate — the same threshold the
        // on-chain matcher applies at flush time to decide whether a side is
        // materialized or thrown out. Routing every builder through it keeps
        // an honest client from emitting a side the engine would silently
        // skip (a no-fill). `level_to_relative` already floors each level and
        // rejects any single one `> BPS`; this bounds the per-side sum.
        profile
            .validate_size_sums()
            .map_err(|_| QuotingError::SizeExceedsInventory)?;
        Ok(profile)
    }

    /// Translate and serialize to the `[u8; 160]` `profile_bytes` arg for
    /// `set_liquidity_profile`.
    pub fn to_profile_bytes(
        &self,
        reference: Price,
        base_atoms: u64,
        quote_atoms: u64,
    ) -> Result<[u8; 160], QuotingError> {
        Ok(profile_bytes(&self.to_profile(
            reference,
            base_atoms,
            quote_atoms,
        )?))
    }
}

/// Serialize a [`LiquidityProfile`] to the `[u8; 160]` instruction arg.
pub fn profile_bytes(profile: &LiquidityProfile) -> [u8; 160] {
    let mut out = [0u8; 160];
    out.copy_from_slice(bytemuck::bytes_of(profile));
    out
}

/// Build the `set_reference_price` instruction (relative-quoting hot path).
///
/// `quote_slot` is taken as the natural `u64` RPC slot and narrowed to the
/// on-chain `u32` field here — the single truncation boundary. The
/// horizon before a live slot exceeds `u32::MAX` is ~a decade (tracked as
/// a follow-up to widen the field); until then the cast is lossless.
pub fn set_reference_price_ix(
    signer: Pubkey,
    market: Pubkey,
    vault_idx: u32,
    reference: Price,
    quote_slot: u64,
) -> Instruction {
    SetReferencePrice { signer, market }.instruction(SetReferencePriceInstructionArgs {
        vault_idx,
        price_bits: reference.as_u32(),
        quote_slot: quote_slot as u32,
    })
}

/// Build the `set_liquidity_profile` instruction from a serialized profile.
pub fn set_liquidity_profile_ix(
    signer: Pubkey,
    market: Pubkey,
    vault_idx: u32,
    profile_bytes: [u8; 160],
) -> Instruction {
    SetLiquidityProfile { signer, market }.instruction(SetLiquidityProfileInstructionArgs {
        vault_idx,
        profile_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_book_translates_to_offsets_and_bps() {
        let reference = Price::encode(10_000_000, 0).unwrap(); // 1.0
        let book = NativeBook {
            asks: vec![NativeLevel {
                price: Price::encode(10_100_000, 0).unwrap(), // 1.01 -> +10000 ppm
                size: 100_000,                                // of 1_000_000 base -> 1000 bps
                expiry_offset: 150,
            }],
            bids: vec![NativeLevel {
                price: Price::encode(99_000_000, -1).unwrap(), // 0.99 -> -10000 ppm
                size: 200_000,                                 // of 1_000_000 quote -> 2000 bps
                expiry_offset: 150,
            }],
        };
        let p = book.to_profile(reference, 1_000_000, 1_000_000).unwrap();
        assert_eq!(p.asks[0].price_offset.get(), 10_000);
        assert_eq!(p.asks[0].size_bps.get(), 1_000);
        assert_eq!(p.asks[0].expiry_offset.get(), 150);
        assert_eq!(p.bids[0].price_offset.get(), 10_000);
        assert_eq!(p.bids[0].size_bps.get(), 2_000);
        // Unused level slots stay zeroed.
        assert_eq!(p.asks[1].size_bps.get(), 0);

        // Serializes to the 160-byte instruction arg.
        let bytes = book
            .to_profile_bytes(reference, 1_000_000, 1_000_000)
            .unwrap();
        assert_eq!(bytes.len(), 160);
    }

    #[test]
    fn rejects_ask_below_reference() {
        let reference = Price::encode(10_000_000, 0).unwrap(); // 1.0
        let book = NativeBook {
            asks: vec![NativeLevel {
                price: Price::encode(99_000_000, -1).unwrap(), // 0.99 < ref
                size: 1,
                expiry_offset: 0,
            }],
            ..Default::default()
        };
        assert_eq!(
            book.to_profile(reference, 1_000_000, 1_000_000)
                .unwrap_err(),
            QuotingError::AskBelowReference
        );
    }

    #[test]
    fn rejects_oversized_side() {
        let reference = Price::encode(10_000_000, 0).unwrap();
        // Two asks each 60% of base -> 120% > 10000 bps per side.
        let lvl = |price| NativeLevel {
            price,
            size: 600_000,
            expiry_offset: 0,
        };
        let book = NativeBook {
            asks: vec![
                lvl(Price::encode(10_100_000, 0).unwrap()),
                lvl(Price::encode(10_200_000, 0).unwrap()),
            ],
            ..Default::default()
        };
        assert_eq!(
            book.to_profile(reference, 1_000_000, 1_000_000)
                .unwrap_err(),
            QuotingError::SizeExceedsInventory
        );
    }

    #[test]
    fn per_side_sum_at_bps_exactly_is_accepted() {
        // Two asks 60% + 40% of base = 100% = 10_000 bps exactly — the strict
        // gate accepts `Σ == BPS`, so an honest client sizing to full commit
        // is never silently skipped by the chain.
        let reference = Price::encode(10_000_000, 0).unwrap();
        let book = NativeBook {
            asks: vec![
                NativeLevel {
                    price: Price::encode(10_100_000, 0).unwrap(),
                    size: 600_000,
                    expiry_offset: 0,
                },
                NativeLevel {
                    price: Price::encode(10_200_000, 0).unwrap(),
                    size: 400_000,
                    expiry_offset: 0,
                },
            ],
            ..Default::default()
        };
        let p = book.to_profile(reference, 1_000_000, 1_000_000).unwrap();
        let ask: u32 = p.asks.iter().map(|l| l.size_bps.get() as u32).sum();
        assert_eq!(ask, 10_000);
    }

    #[test]
    fn size_bps_floors_and_never_rounds_up_past_bps() {
        // 999_999 / 1_000_000 = 99.9999% → 9999.99 bps, which floors to 9999,
        // never rounding up to 10_000. Flooring is what keeps a per-side sum
        // from overshooting the cap.
        let reference = Price::encode(10_000_000, 0).unwrap();
        let book = NativeBook {
            asks: vec![NativeLevel {
                price: Price::encode(10_100_000, 0).unwrap(),
                size: 999_999,
                expiry_offset: 0,
            }],
            ..Default::default()
        };
        let p = book.to_profile(reference, 1_000_000, 1_000_000).unwrap();
        assert_eq!(p.asks[0].size_bps.get(), 9_999);
    }

    #[test]
    fn too_many_levels_rejected() {
        let reference = Price::encode(10_000_000, 0).unwrap();
        let book = NativeBook {
            asks: (0..=N_LEVELS)
                .map(|_| NativeLevel {
                    price: Price::encode(10_100_000, 0).unwrap(),
                    size: 1,
                    expiry_offset: 0,
                })
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            book.to_profile(reference, 1_000_000, 1_000_000)
                .unwrap_err(),
            QuotingError::TooManyLevels
        );
    }
}
