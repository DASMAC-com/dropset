//! `override_vault_name` — admin rewrite of a vault's name, with a lock.
//!
//! The abuse bound on leader-chosen names: an admin writes any name the
//! shared rules accept ([`crate::state::encode_vault_name`]), blank
//! included, under the same per-market uniqueness walk, and sets
//! `Vault.name_locked = lock`. A locked name refuses the leader's
//! `set_vault_name` until an admin unlocks it. Admin-only: authorized via
//! the registry admin set, in the `freeze_vault` account shape. Applies to
//! a frozen vault too — the admin path is not refused where the leader's
//! is. See the architecture spec § OverrideVaultName.

use anchor_lang::prelude::*;

use crate::{
    errors::DropsetError,
    events::OverrideVaultNameEvent,
    state::{decode_vault_name, encode_vault_name, Market, VaultAccess},
    Registry,
};

#[event_cpi]
#[derive(Accounts)]
pub struct OverrideVaultName {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market holding the target vault. `mut` for the name write.
    #[account(mut)]
    pub market: Market,
}

impl OverrideVaultName {
    /// Returns the [`OverrideVaultNameEvent`] payload for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn override_vault_name(
        &mut self,
        vault_idx: u32,
        name: String,
        lock: bool,
    ) -> Result<OverrideVaultNameEvent> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        let packed = encode_vault_name(&name)?;
        let (vault_seq, old_name) = {
            let vault = self.market.read_vault(vault_idx)?;
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            (vault.seq.get(), decode_vault_name(&vault.name))
        };
        require!(
            !self.market.name_taken(&packed, vault_idx),
            DropsetError::VaultNameTaken
        );

        let vault = self.market.mutate_vault(vault_idx)?;
        vault.name = packed;
        vault.name_locked = lock.into();

        Ok(OverrideVaultNameEvent {
            market: *self.market.address(),
            sector_idx: vault_idx,
            vault_seq,
            admin: *self.admin.address(),
            lock,
            old_name,
            new_name: name,
        })
    }
}
