//! `set_reference_price` — leader hot path.
//!
//! Updates the vault's reference price (and `quote_slot`) in two
//! aligned `u64` stores, bumps `market.nonce`, and arms the
//! `FLUSH_BIT` so the next taker materializes `Vault.remaining` from
//! the (unchanged) `LiquidityProfile`. Per the architecture spec's
//! **SetReferencePrice**, this is the asm-optimized path — analogous
//! to a propAMM reference-price update — so it emits no events and
//! does no list walks.

use anchor_lang_v2::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    state::{Market, FLUSH_BIT, MAX_BACKDATE},
    Price, ReferencePrice,
};

#[derive(Accounts)]
pub struct SetReferencePrice {
    /// Quote authority — the only signer the spec accepts here. Set at
    /// `OpenVault` (defaults to the leader).
    pub signer: Signer,
    /// Market account holding the target vault.
    #[account(mut)]
    pub market: Market,
    /// Read for the current slot. The leader-supplied `quote_slot` is
    /// validated against this (no future-dating, capped backdate).
    pub clock: Sysvar<Clock>,
}

impl SetReferencePrice {
    #[inline(always)]
    pub fn set_reference_price(
        &mut self,
        vault_idx: u32,
        price: Price,
        quote_slot: u64,
    ) -> Result<()> {
        // Validate the price bit pattern up front — `Price` derives Pod
        // so Anchor will deserialize any 4-byte input; an invalid
        // significand would mis-sort in the matching engine.
        require!(price.is_valid(), DropsetError::InvalidPrice);

        let len = self.market.len();
        require!(
            (vault_idx as usize) < len,
            DropsetError::InvalidSectorIndex
        );

        // Bump the market nonce first (this borrows the header via
        // Slab's DerefMut). The tail borrow that mutates the vault
        // happens after this header write completes.
        let nonce = self.market.nonce.get();
        let new_nonce = nonce
            .checked_add(1)
            .ok_or(DropsetError::MathOverflow)?;
        self.market.nonce = new_nonce.into();

        let current_slot = self.clock.slot;
        require!(
            quote_slot <= current_slot
                && current_slot.saturating_sub(quote_slot) <= MAX_BACKDATE,
            DropsetError::InvalidQuoteSlot
        );

        // Now mutate the vault sector.
        let signer_addr = *self.signer.address();
        let vault = &mut self.market.as_mut_slice()[vault_idx as usize];
        require!(
            !address_eq(&vault.leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(
            address_eq(&vault.quote_authority, &signer_addr),
            DropsetError::Unauthorized
        );
        require!(vault.frozen == 0, DropsetError::VaultFrozen);

        vault.reference_price = ReferencePrice {
            stamp: (nonce | FLUSH_BIT).into(),
            price,
            quote_slot: (quote_slot as u32).into(),
        };
        Ok(())
    }
}
