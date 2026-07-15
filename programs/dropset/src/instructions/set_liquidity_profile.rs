//! `set_liquidity_profile` — leader-driven reshape of the bid/ask
//! ladder.
//!
//! Writes the full [`LiquidityProfile`] — each level a `(price_offset,
//! size_bps, expiry_offset)` triple — leaves `reference_price.price`
//! and `reference_price.quote_slot` untouched, bumps `market.nonce`,
//! and arms `FLUSH_BIT` so the next taker re-materializes
//! `Vault.remaining` from the new ladder + current inventory.
//!
//! **Pre-condition**: rejects when
//! `vault.reference_price.price.is_zero()` — i.e. before the leader
//! has called `set_reference_price` at least once. The profile is
//! pure-relative; without an anchor price the offsets have no
//! meaning. See the spec's **SetLiquidityProfile**.

use anchor_lang_v2::{address_eq, bytemuck, prelude::*};

use crate::{
    errors::DropsetError,
    state::{Market, VaultAccess, BPS, FLUSH_BIT},
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

        // Per-side `Σ size_bps ≤ BPS`, rejected here before any `profile`
        // bytes are stored. Shares the summation with the match-time flush
        // gate in `Vault::materialize_remaining` via
        // `LiquidityProfile::side_size_sums`; this write-time path rejects
        // an over-BPS profile outright, whereas the flush path zeroes the
        // offending side out of matching.
        let (bid_sum, ask_sum) = profile.side_size_sums();
        require!(
            bid_sum <= BPS as u32 && ask_sum <= BPS as u32,
            DropsetError::LiquidityProfileSizeOverflow
        );

        // Validate the target vault BEFORE bumping `market.nonce`. A
        // caller targeting a free-list sector or the wrong vault must
        // not advance the header counter.
        let signer_addr = *self.signer.address();
        {
            let vault = self.market.read_vault(vault_idx)?;
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            require!(
                address_eq(&vault.quote_authority, &signer_addr),
                DropsetError::Unauthorized
            );
            // Reject a frozen vault here even though the sibling
            // `set_reference_price` path does not: that path's ASM kernel
            // stays minimal (the freeze is ultimately enforced at match
            // time, where `swap` skips frozen vaults), whereas this handler
            // already reads the vault, so the guard is near-free and fails
            // fast.
            require!(!vault.frozen.get(), DropsetError::VaultFrozen);
            // Per-spec rule: a vault's reference price must be
            // set before its profile is — the profile is pure ppm
            // offsets and needs a real anchor.
            require!(
                !vault.reference_price.price.is_zero(),
                DropsetError::ReferencePriceNotSet
            );
        }

        // Bump market.nonce (header borrow). `checked_add` here — same as
        // `swap`; the `set_reference_price` ASM kernel is the one path that
        // `wrapping_add`s instead (see `state/market/reference_price.rs`).
        let nonce = self.market.nonce.get();
        let new_nonce = nonce.checked_add(1).ok_or(DropsetError::MathOverflow)?;
        self.market.nonce = new_nonce.into();

        // Re-borrow the vault mutably and stamp the new profile.
        let vault = self.market.mutate_vault(vault_idx)?;
        vault.profile = *profile;
        // Stamp the new nonce | FLUSH_BIT; leave `price` and
        // `quote_slot` untouched — that's the SetLiquidityProfile
        // contract per spec.
        vault.reference_price.stamp = (nonce | FLUSH_BIT).into();
        Ok(())
    }
}
