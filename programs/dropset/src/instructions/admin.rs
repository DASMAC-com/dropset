use crate::errors::DropsetError;
use crate::{AdminSet, Registry};
use anchor_lang_v2::prelude::*;

/// Add a pubkey to the registry admin set. Authorized by any existing
/// admin (the `admin` signer), who also funds the extra rent for the
/// grown account.
#[derive(Accounts)]
pub struct AddAdmin {
    #[account(mut)]
    pub admin: Signer,
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    pub system_program: Program<System>,
}

impl AddAdmin {
    #[inline(always)]
    pub fn add_admin(&mut self, new_admin: Address) -> Result<()> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        self.registry.admin_insert(new_admin, self.admin.as_ref())?;
        Ok(())
    }
}

/// Remove a pubkey from the registry admin set. Authorized by any
/// existing admin; the freed rent is refunded to the `admin` signer.
/// The last remaining admin cannot be removed.
#[derive(Accounts)]
pub struct RemoveAdmin {
    #[account(mut)]
    pub admin: Signer,
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
}

impl RemoveAdmin {
    #[inline(always)]
    pub fn remove_admin(&mut self, target: Address) -> Result<()> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        // A standalone view onto the signer account so the freed rent
        // can be credited back to it (see `Slab::refund`).
        let mut rent_recipient = *self.admin.as_ref();
        if !self.registry.admin_remove(&target, &mut rent_recipient)? {
            return Err(DropsetError::AdminNotFound.into());
        }
        Ok(())
    }
}
