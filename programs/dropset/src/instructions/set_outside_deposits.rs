//! The two-key gate toggles for outside deposits.
//!
//! Outside `Deposit` requires both halves of a two-key gate to be set
//! (architecture spec § Deposit, § Leader/Admin operations):
//!
//! * `set_allow_outside_depositors` — **leader-only**. Flips
//!   `Vault.allow_outside_depositors` (§ SetAllowOutsideDepositors).
//! * `set_outside_deposits_approved` — **admin-only**. Flips
//!   `Vault.outside_deposits_approved` (§ SetOutsideDepositsApproved).
//!
//! New vaults open with both flags `false` (the `register_vault`
//! default), so outside deposits are unreachable until each side
//! opts in. Flipping either flag back to `false` blocks **new** outside
//! deposits but leaves existing depositors' `withdraw` path open — the
//! deposit-side gate is the only place these flags are read.
//!
//! Neither setter touches the reference price, the ladder, or
//! `market.nonce`: they mutate a single config bool, so there is no
//! flush to arm and no matching-engine state to invalidate.

use anchor_lang_v2::{address_eq, prelude::*};

use crate::{errors::DropsetError, state::Market, AdminSet, Registry};

#[derive(Accounts)]
pub struct SetAllowOutsideDepositors {
    /// The vault's leader — the only signer this setter accepts.
    pub signer: Signer,
    /// Market holding the target vault.
    #[account(mut)]
    pub market: Market,
}

impl SetAllowOutsideDepositors {
    #[inline(always)]
    pub fn set_allow_outside_depositors(&mut self, vault_idx: u32, flag: bool) -> Result<()> {
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Validate the target vault through an immutable borrow before
        // taking the mutable one — the house pattern shared with
        // `set_reference_price` / `set_liquidity_profile`. Authorization
        // is read-only, so it stays out of the `&mut` scope, and the
        // mutable borrow narrows to the single store below.
        let signer_addr = *self.signer.address();
        {
            let vault = &self.market.as_slice()[vault_idx as usize];
            // A free-list sector carries `leader == default`; reject it
            // first so the error names the real cause rather than
            // surfacing as an authorization failure.
            require!(
                !address_eq(&vault.leader, &Address::default()),
                DropsetError::VaultEmpty
            );
            // Leader-only — deliberately distinct from the
            // `quote_authority` gate on the quoting setters. Opening a
            // vault to outside capital is a custody decision, so it
            // stays with the leader even when quoting is delegated.
            require!(
                address_eq(&vault.leader, &signer_addr),
                DropsetError::Unauthorized
            );
        }

        self.market.as_mut_slice()[vault_idx as usize].allow_outside_depositors = flag.into();
        Ok(())
    }
}

#[derive(Accounts)]
pub struct SetOutsideDepositsApproved {
    /// Registry admin — authorized via the registry admin set.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market holding the target vault.
    #[account(mut)]
    pub market: Market,
}

impl SetOutsideDepositsApproved {
    #[inline(always)]
    pub fn set_outside_deposits_approved(&mut self, vault_idx: u32, flag: bool) -> Result<()> {
        // Admin-only — the protocol's half of the two-key gate. The
        // registry account is PDA-pinned (`seeds = [b"registry"]`), so
        // membership is checked against the canonical admin set.
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        // Same validate-then-mutate shape as the leader setter above:
        // confirm the sector is live through an immutable borrow, then
        // narrow to the single store.
        {
            let vault = &self.market.as_slice()[vault_idx as usize];
            require!(
                !address_eq(&vault.leader, &Address::default()),
                DropsetError::VaultEmpty
            );
        }

        self.market.as_mut_slice()[vault_idx as usize].outside_deposits_approved = flag.into();
        Ok(())
    }
}
