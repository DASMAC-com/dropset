//! `set_quote_authority` — leader-only quote-authority rotation.
//!
//! Rewrites a vault's `quote_authority` — the delegate that signs the
//! quoting hot path (`SetReferencePrice` / `SetLiquidityProfile`). Per
//! the architecture spec § SetQuoteAuthority the gate is **leader-only**:
//! the new authority may be any pubkey, including the leader's own
//! (effectively revoking delegation). Useful for rotating a hot wallet,
//! delegating to a third-party MM firm, or moving quoting authority while
//! keeping custody of inventory.
//!
//! Like the other leader-only config setters (`set_allow_outside_depositors`,
//! `close_vault`) this mutates a single header field: no reference price,
//! no ladder, and no `market.nonce` flush to arm, so there is no
//! matching-engine state to invalidate. Per the spec's **Events and
//! emission** governing principle it emits **nothing** — the whole effect
//! is recoverable from a one-field account diff.

use anchor_lang_v2::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    state::{Market, VaultAccess},
};

#[derive(Accounts)]
pub struct SetQuoteAuthority {
    /// The vault's leader — the only signer this rotation accepts.
    pub signer: Signer,
    /// Market holding the target vault. `mut` for the authority write.
    #[account(mut)]
    pub market: Market,
}

impl SetQuoteAuthority {
    #[inline(always)]
    pub fn set_quote_authority(&mut self, vault_idx: u32, new_authority: Address) -> Result<()> {
        // Reject the zero address — the same quote-brick guard
        // `create_vault` applies to the initial `quote_authority`. The
        // zero pubkey doubles as the free-list emptiness marker and has no
        // private key, so stamping it here would brick the quoting hot
        // path (`SetReferencePrice` / `SetLiquidityProfile` gate on
        // `signer == quote_authority`). A leader who wants no separate
        // delegation passes their own pubkey, not a default sentinel.
        require!(
            !address_eq(&new_authority, &Address::default()),
            DropsetError::Unauthorized
        );

        // Validate the target vault through an immutable borrow before
        // taking the mutable one — the house validate-then-mutate shape
        // shared with the other vault setters. Authorization is read-only,
        // so it stays out of the `&mut` scope, and the mutable borrow
        // narrows to the single store below.
        let signer_addr = *self.signer.address();
        {
            let vault = self.market.read_vault(vault_idx)?;
            // A free-list sector carries `leader == default`; reject it
            // first so the error names the real cause rather than
            // surfacing as an authorization failure.
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            // Leader-only — deliberately the leader, never the incumbent
            // `quote_authority`. Rotating the quoting delegate is a custody
            // decision the leader owns; letting the delegate rotate it
            // would let a third-party MM lock the leader out of its vault.
            require!(
                address_eq(&vault.leader, &signer_addr),
                DropsetError::Unauthorized
            );
        }

        self.market.mutate_vault(vault_idx)?.quote_authority = new_authority;
        Ok(())
    }
}
