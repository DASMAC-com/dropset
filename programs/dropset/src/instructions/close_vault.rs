//! `close_vault` — leader-initiated lifecycle exit.
//!
//! Moves the vault from the active DLL to the tombstone DLL: the
//! matching engine stops visiting it (tombstones are not iterated), but
//! depositor `withdraw` flows stay open until the vault drains. The
//! sector keeps all its data; only the list membership and
//! `active_count` change. Per the architecture spec's **CloseVault**
//! and **Vault → Frozen and tombstoned vaults**, this is the leader's
//! intended "done quoting this market" path — distinct from admin
//! `freeze_vault`, which leaves the vault on the active DLL so existing
//! levels keep matching until they expire.
//!
//! Leader-only: rejects when `signer != vault.leader`. A vault that is
//! already tombstoned is rejected (`VaultAlreadyTombstoned`); a
//! free-list sector is rejected as empty.

use anchor_lang_v2::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    events::CloseVaultEvent,
    state::{DllList, Market, VaultDll},
};

#[event_cpi]
#[derive(Accounts)]
pub struct CloseVault {
    /// Must equal `vault.leader` — verified in-handler.
    pub signer: Signer,
    /// Market holding the target vault. `mut` because the active /
    /// tombstone list heads and `active_count` change.
    #[account(mut)]
    pub market: Market,
}

impl CloseVault {
    /// Returns the [`CloseVaultEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!` (see [`super::register_vault`] for why the
    /// emit lives in the dispatcher rather than the handler).
    #[inline(always)]
    pub fn close_vault(&mut self, vault_idx: u32) -> Result<CloseVaultEvent> {
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let signer_addr = *self.signer.address();
        let leader = {
            let v = &self.market.as_slice()[vault_idx as usize];
            v.leader
        };
        // Free-list sectors carry the default leader — reject before the
        // leader check so the error is specific.
        require!(
            !address_eq(&leader, &Address::default()),
            DropsetError::VaultEmpty
        );
        require!(address_eq(&leader, &signer_addr), DropsetError::Unauthorized);

        // The vault must currently be on the active DLL. A vault already
        // on the tombstone list is a no-op the leader probably didn't
        // intend; a sector that's on neither (free / detached) would
        // have been caught by the `VaultEmpty` check above.
        match self.market.vault_list_of(vault_idx) {
            Some(DllList::Active) => {}
            Some(DllList::Tombstone) => {
                return Err(DropsetError::VaultAlreadyTombstoned.into())
            }
            _ => return Err(DropsetError::CorruptVaultList.into()),
        }

        // Active → tombstone: unlink, drop the active count, re-thread.
        self.market.unlink(DllList::Active, vault_idx)?;
        let prev = self.market.active_count.get();
        let active_count_after = prev.saturating_sub(1);
        self.market.active_count = active_count_after.into();
        self.market.link_head(DllList::Tombstone, vault_idx)?;

        Ok(CloseVaultEvent {
            market: *self.market.address(),
            sector_idx: vault_idx,
            leader,
            active_count_after,
        })
    }
}
