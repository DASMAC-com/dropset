//! `freeze_vault` — admin revocation lever against a misbehaving leader.
//!
//! Sets `Vault.frozen = true`. The vault stays on the active DLL, so
//! existing materialized levels keep matching until their `expires_at`,
//! but it can no longer be re-quoted: `set_reference_price` /
//! `set_liquidity_profile` reject frozen vaults, and `realize_in_place`
//! pins the HWM at freeze time. Per the spec's **FreezeVault**, there is
//! no "unfreeze" — to re-enter, the same leader opens a fresh vault.
//!
//! Admin-only: authorized via the registry admin set, mirroring
//! `set_outside_deposits_approved`. Freezing an already-frozen vault is
//! an idempotent no-op (the flag is simply re-asserted).

use anchor_lang_v2::prelude::*;

use crate::{
    errors::DropsetError,
    events::FreezeVaultEvent,
    state::{Market, VaultAccess},
    AdminSet, Registry,
};

#[event_cpi]
#[derive(Accounts)]
pub struct FreezeVault {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market holding the target vault. `mut` for the `frozen` write.
    #[account(mut)]
    pub market: Market,
}

impl FreezeVault {
    /// Returns the [`FreezeVaultEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`.
    #[inline(always)]
    pub fn freeze_vault(&mut self, vault_idx: u32) -> Result<FreezeVaultEvent> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        let market_addr = *self.market.address();
        let vault = self.market.mutate_vault(vault_idx)?;
        require!(vault.is_occupied(), DropsetError::VaultEmpty);
        let leader = vault.leader;
        vault.frozen = true.into();

        Ok(FreezeVaultEvent {
            market: market_addr,
            sector_idx: vault_idx,
            leader,
        })
    }
}
