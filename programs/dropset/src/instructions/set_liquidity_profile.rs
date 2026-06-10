//! `set_liquidity_profile` — leader-driven reshape of the bid/ask
//! ladder.
//!
//! Writes the full [`LiquidityProfile`] — each level a `(price_offset,
//! size_bps, expiry_offset)` triple — leaves `reference_price.price`
//! and `reference_price.quote_slot` untouched, bumps `market.nonce`,
//! and arms `FLUSH_BIT` so the next taker re-materializes
//! `Vault.remaining` from the new ladder + current inventory.
//!
//! **MVP pre-condition** (added in ENG-423): rejects when
//! `vault.reference_price.price.is_zero()` — i.e. before the leader
//! has called `set_reference_price` at least once. The profile is
//! pure-relative; without an anchor price the offsets have no
//! meaning. See the spec's **SetLiquidityProfile**.

use anchor_lang_v2::{address_eq, bytemuck, prelude::*};

use crate::{
    errors::DropsetError,
    state::{Market, BPS, FLUSH_BIT},
    LiquidityProfile, N_LEVELS,
};

/// On-wire byte representation of [`LiquidityProfile`]. The struct is
/// alignment-1 Pod (`#[repr(C)]` plus 1-byte fields), so an instruction
/// arg of this width casts back via `bytemuck::from_bytes` without
/// rewriting the layout. Sized via `size_of` so the compile-time guard
/// in `state::market.rs` (`size_of::<LiquidityProfile>() == 2 *
/// N_LEVELS * 10`) and this stay locked together.
pub const PROFILE_BYTES: usize = 2 * N_LEVELS * 10;

#[derive(Accounts)]
pub struct SetLiquidityProfile {
    /// Quote authority — same gate as `set_reference_price`.
    pub signer: Signer,
    /// Market account holding the target vault.
    #[account(mut)]
    pub market: Market,
}

impl SetLiquidityProfile {
    #[inline(always)]
    pub fn set_liquidity_profile(
        &mut self,
        vault_idx: u32,
        profile_bytes: [u8; PROFILE_BYTES],
    ) -> Result<()> {
        let profile: &LiquidityProfile = bytemuck::from_bytes(&profile_bytes);
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Per-side `Σ size_bps ≤ 10_000`. `u32` accumulator: at
        // N_LEVELS = 8 the upper bound is `8 × u16::MAX = 524_280`,
        // far inside `u32` range.
        let mut bid_sum: u32 = 0;
        let mut ask_sum: u32 = 0;
        for i in 0..N_LEVELS {
            bid_sum = bid_sum.saturating_add(profile.bids[i].size_bps.get() as u32);
            ask_sum = ask_sum.saturating_add(profile.asks[i].size_bps.get() as u32);
        }
        require!(
            bid_sum <= BPS as u32 && ask_sum <= BPS as u32,
            DropsetError::LiquidityProfileSizeOverflow
        );

        // Validate the target vault BEFORE bumping `market.nonce`. A
        // caller targeting a free-list sector or the wrong vault must
        // not advance the header counter.
        let signer_addr = *self.signer.address();
        {
            let vault = &self.market.as_slice()[vault_idx as usize];
            require!(
                !address_eq(&vault.leader, &Address::default()),
                DropsetError::VaultEmpty
            );
            require!(
                address_eq(&vault.quote_authority, &signer_addr),
                DropsetError::Unauthorized
            );
            require!(vault.frozen == 0, DropsetError::VaultFrozen);
            // The MVP rule (ENG-423): a vault's reference price must be
            // set before its profile is — the profile is pure ppm
            // offsets and needs a real anchor.
            require!(
                !vault.reference_price.price.is_zero(),
                DropsetError::ReferencePriceNotSet
            );
        }

        // Bump market.nonce (header borrow).
        let nonce = self.market.nonce.get();
        let new_nonce = nonce.checked_add(1).ok_or(DropsetError::MathOverflow)?;
        self.market.nonce = new_nonce.into();

        // Re-borrow the vault mutably and stamp the new profile.
        let vault = &mut self.market.as_mut_slice()[vault_idx as usize];
        vault.profile = *profile;
        // Stamp the new nonce | FLUSH_BIT; leave `price` and
        // `quote_slot` untouched — that's the SetLiquidityProfile
        // contract per spec.
        vault.reference_price.stamp = (nonce | FLUSH_BIT).into();
        Ok(())
    }
}
