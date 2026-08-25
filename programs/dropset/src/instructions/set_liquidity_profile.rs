//! `set_liquidity_profile` — leader-driven reshape of the bid/ask
//! ladder.
//!
//! Writes the full `LiquidityProfile` — each level a `(price_offset,
//! size_bps, expiry_offset_secs, expiry_offset_slots)` tuple — leaves
//! the whole `reference_price` untouched (price and both expiry datums),
//! bumps `market.nonce`,
//! and arms `FLUSH_BIT` so the next taker re-materializes
//! `Vault.remaining` from the new ladder + current inventory. Per the
//! architecture spec's **SetLiquidityProfile** the profile bytes are stored
//! raw: the only domain guard is that the signer is the vault's
//! `quote_authority`, and every content invariant is enforced at match time
//! (an over-cap side is dropped from the book, and a ladder armed before a
//! reference price never materializes at all).
//!
//! Two builds share one implementation, exactly as `set_reference_price`
//! does. The production `asm-entrypoint` build handles this discriminator
//! entirely in `src/asm/entrypoint.s`, so the Rust body here is an
//! `unreachable_unchecked()` stub kept only so IDL / SDK codegen still emit
//! the instruction. The default (reference) build runs this handler, which
//! borrows the market's data bytes and calls the shared
//! `write_liquidity_profile` kernel — the same kernel the assembly
//! mirrors byte-for-byte.

use anchor_lang_v2::prelude::*;

#[cfg(not(feature = "asm-entrypoint"))]
use crate::state::write_liquidity_profile;

/// On-wire byte representation of [`crate::LiquidityProfile`]. The struct is
/// alignment-1 Pod (`#[repr(C)]` plus 1-byte fields), so an instruction
/// arg of this width casts back via `bytemuck::from_bytes` without
/// rewriting the layout. Aliases the kernel's `PROFILE_SIZE` — the width
/// the ASM hands `sol_memcpy_` — so the instruction arg and the copy length
/// are one value by construction rather than two derivations held equal by
/// assertion.
pub const PROFILE_BYTES: usize = crate::state::PROFILE_SIZE;

#[derive(Accounts)]
pub struct SetLiquidityProfile {
    /// Quote authority — same gate as `set_reference_price`.
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

impl SetLiquidityProfile {
    #[inline(always)]
    pub fn set_liquidity_profile(
        &mut self,
        vault_idx: u32,
        profile_bytes: [u8; PROFILE_BYTES],
    ) -> Result<()> {
        #[cfg(feature = "asm-entrypoint")]
        {
            // The asm entrypoint writes this discriminator before the
            // anchor dispatcher runs, so this body is never reached. Kept
            // as a stub purely so IDL / SDK codegen still emit the
            // instruction interface.
            let _ = (vault_idx, profile_bytes);
            unsafe { core::hint::unreachable_unchecked() }
        }
        #[cfg(not(feature = "asm-entrypoint"))]
        {
            // `Address` is a 32-byte `Pod` wrapper; reinterpret it as the
            // raw key bytes the kernel compares, without depending on its
            // inherent accessors.
            let signer_key: &[u8; 32] = anchor_lang_v2::bytemuck::cast_ref(self.signer.address());
            // `AccountView` is `Copy` and borrow state lives in the shared
            // account header, so a local copy still tracks the one live
            // mutable borrow of the market's data.
            let mut view = *self.market.account();
            let mut data = view.try_borrow_mut()?;
            write_liquidity_profile(&mut data, vault_idx, &profile_bytes, signer_key)
                .map_err(ProgramError::Custom)?;
            Ok(())
        }
    }
}
