//! `set_vault_name` — leader-only rename of a vault's display label.
//!
//! Writes `Vault.name` under the shared rules
//! ([`crate::state::encode_vault_name`]: printable ASCII, at most 32
//! bytes, empty means unset) and the per-market, case-insensitive
//! uniqueness walk. Refused on a frozen vault, on a tombstoned one, and on
//! one whose name an admin locked through `override_vault_name`. The
//! tombstone refusal is what makes `close_vault`'s release final: close
//! blanks the name and clears the lock, so a leader renaming the tombstone
//! afterwards would both re-take a released name and shed an admin lock.
//! The vault's identity is
//! `Vault::seq`, so a rename changes a label and nothing else. See the
//! architecture spec § SetVaultName.

use anchor_lang::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    events::SetVaultNameEvent,
    state::{decode_vault_name, encode_vault_name, Market, VaultAccess},
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetVaultName {
    /// The vault's leader — the only signer this rename accepts.
    pub signer: Signer,
    /// Market holding the target vault. `mut` for the name write.
    #[account(mut)]
    pub market: Market,
}

impl SetVaultName {
    /// Returns the [`SetVaultNameEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`.
    #[inline(always)]
    pub fn set_vault_name(&mut self, vault_idx: u32, name: String) -> Result<SetVaultNameEvent> {
        let packed = encode_vault_name(&name)?;

        // Validate through an immutable borrow, then take the mutable one
        // for the single store — the house validate-then-mutate shape.
        let signer_addr = *self.signer.address();
        let (vault_seq, old_name) = {
            let vault = self.market.read_vault(vault_idx)?;
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            require!(
                address_eq(&vault.leader, &signer_addr),
                DropsetError::Unauthorized
            );
            require!(!vault.frozen.get(), DropsetError::VaultFrozen);
            require!(!vault.tombstoned.get(), DropsetError::VaultTombstoned);
            require!(!vault.name_locked.get(), DropsetError::VaultNameLocked);
            (vault.seq.get(), decode_vault_name(&vault.name))
        };
        require!(
            !self.market.name_taken(&packed, vault_idx),
            DropsetError::VaultNameTaken
        );

        self.market.mutate_vault(vault_idx)?.name = packed;

        Ok(SetVaultNameEvent {
            market: *self.market.address(),
            sector_idx: vault_idx,
            vault_seq,
            leader: signer_addr,
            old_name,
            new_name: name,
        })
    }
}
