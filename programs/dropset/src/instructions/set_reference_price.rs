//! `set_reference_price` — leader hot path.
//!
//! Stamps the target vault's reference price (and `quote_slot`), bumps
//! `market.nonce`, and arms the `FLUSH_BIT` so the next taker
//! materializes `Vault.remaining` from the (unchanged) `LiquidityProfile`.
//! Per the architecture spec's **SetReferencePrice** this is the
//! asm-optimized path: it emits no events, walks no lists, and stores the
//! price / slot raw — matching skips an invalid-price vault, so there is
//! no write-time validation to do (the only domain guard is that the
//! signer is the vault's `quote_authority`).
//!
//! Two builds share one implementation. The production `asm-entrypoint`
//! build handles this discriminator entirely in `src/asm/entrypoint.s`,
//! so the Rust body here is an `unreachable_unchecked()` stub kept only so
//! IDL / SDK codegen still emit the instruction. The default (reference)
//! build runs this handler, which borrows the market's data bytes and
//! calls the shared [`stamp_reference_price`] kernel — the same kernel the
//! assembly mirrors byte-for-byte.

use anchor_lang_v2::prelude::*;

#[cfg(not(feature = "asm-entrypoint"))]
use crate::state::stamp_reference_price;

#[derive(Accounts)]
pub struct SetReferencePrice {
    /// Quote authority — the only signer the spec accepts here. Set at
    /// `CreateVault` (defaults to the leader); the kernel checks it
    /// against the target vault's `quote_authority`.
    pub signer: Signer,
    /// CHECK: taken unchecked so the handler can borrow the raw account
    /// data and drive the shared kernel (a typed `Market` locks the
    /// account exclusively and would deny that borrow). The account's
    /// discriminator and owner are not re-validated here: the authority
    /// check plus runtime program-ownership at the store are the guards,
    /// exactly as on the asm fast path this build mirrors.
    #[account(mut)]
    pub market: UncheckedAccount,
}

impl SetReferencePrice {
    #[inline(always)]
    pub fn set_reference_price(
        &mut self,
        vault_idx: u32,
        price_bits: u32,
        quote_slot: u32,
    ) -> Result<()> {
        #[cfg(feature = "asm-entrypoint")]
        {
            // The asm entrypoint stamps this discriminator before the
            // anchor dispatcher runs, so this body is never reached. Kept
            // as a stub purely so IDL / SDK codegen still emit the
            // instruction interface.
            let _ = (vault_idx, price_bits, quote_slot);
            unsafe { core::hint::unreachable_unchecked() }
        }
        #[cfg(not(feature = "asm-entrypoint"))]
        {
            // `Address` is a 32-byte `Pod` newtype; reinterpret it as the
            // raw key bytes the kernel compares, without depending on its
            // inherent accessors.
            let signer_key: &[u8; 32] = anchor_lang_v2::bytemuck::cast_ref(self.signer.address());
            // `AccountView` is `Copy` and borrow state lives in the shared
            // account header, so a local copy still tracks the one live
            // mutable borrow of the market's data.
            let mut view = *self.market.account();
            let mut data = view.try_borrow_mut()?;
            stamp_reference_price(&mut data, vault_idx, price_bits, quote_slot, signer_key)
                .map_err(ProgramError::Custom)?;
            Ok(())
        }
    }
}
