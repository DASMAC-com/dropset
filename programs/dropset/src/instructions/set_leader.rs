//! `set_leader` — leader-only custody rotation, co-signed by the new key.
//!
//! Rewrites a vault's `leader` — the key that owns its inventory, its
//! `leader_shares` and every leader-only lever. Per the architecture spec
//! § SetLeader the gate is **leader-only** and the incoming key must
//! **co-sign**, which proves the destination is live at the moment of
//! rotation. The rejected alternative was a two-step pending / accept,
//! which buys the same liveness proof with a stored pending key, a second
//! instruction, cleanup on close and reclaim, and a guard re-run at accept
//! time.
//!
//! Only the `leader` field moves. `quote_authority` deliberately does not
//! follow: the intended endgame is the leader on a hardware wallet and the
//! quoting delegate on the hot maker key, so a caller that wants both
//! rotated bundles a `set_quote_authority` in the same transaction. Shares,
//! HWM, fee rate and flags are untouched, and `seq` is unchanged — this is
//! the same vault under a new key, not a new vault.
//!
//! There is deliberately no admin path: an admin rotation would be a
//! custody-seizure lever, and admins already hold `freeze_vault`. Allowed
//! on tombstoned vaults — moving custody of a winding-down vault is
//! exactly what a leaked-key rotation needs. Rejected on frozen vaults:
//! a frozen vault blocks its leader from opening another on the market,
//! and rotating it onto a second key the leader controls would lift that
//! block. A frozen vault can't trade, so a leaked key on one has nothing
//! to quote.

use anchor_lang_v2::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    events::SetLeaderEvent,
    state::{Market, VaultAccess},
};

#[event_cpi]
#[derive(Accounts)]
#[instruction(vault_idx: u32)]
pub struct SetLeader {
    /// The vault's current leader — the only key that may hand it off.
    pub leader: Signer,
    /// The incoming leader. Its signature is the liveness proof.
    pub new_leader: Signer,
    /// Market holding the target vault. `mut` for the leader write.
    #[account(mut)]
    pub market: Market,
    /// The `VaultDepositor` PDA the new leader would hold on this vault.
    /// Must be uninitialized: a vault's leader never also holds a
    /// depositor position on it (see the handler).
    /// CHECK: bound to `(market, vault_idx, new_leader)` by the seeds and
    /// only read for its owner.
    #[account(
        seeds = [
            b"vault_depositor",
            market.address().as_ref(),
            &vault_idx.to_le_bytes(),
            new_leader.address().as_ref(),
        ],
        bump,
    )]
    pub new_leader_depositor: UncheckedAccount,
}

impl SetLeader {
    /// Returns the [`SetLeaderEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`.
    #[inline(always)]
    pub fn set_leader(&mut self, vault_idx: u32) -> Result<SetLeaderEvent> {
        let old_leader = *self.leader.address();
        let new_leader = *self.new_leader.address();

        // Validate through an immutable borrow before taking the mutable
        // one — the validate-then-mutate shape `set_quote_authority` uses.
        let (vault_seq, quote_authority) = {
            let vault = self.market.read_vault(vault_idx)?;
            // A free-list sector carries `leader == default`; reject it
            // first so the error names the real cause rather than
            // surfacing as an authorization failure.
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            require!(
                address_eq(&vault.leader, &old_leader),
                DropsetError::Unauthorized
            );
            require!(!vault.frozen.get(), DropsetError::VaultFrozen);
            (vault.seq.get(), vault.quote_authority)
        };

        // Reject the zero address. It is the free-list emptiness marker,
        // so a zero leader would put a funded vault on the free list for
        // `create_vault` to reallocate over. No key signs for it, so the
        // co-sign already excludes it; this keeps the invariant local
        // rather than leaning on that.
        require!(
            !address_eq(&new_leader, &Address::default()),
            DropsetError::Unauthorized
        );
        // A no-op rotation would emit an event claiming a hand-off that
        // never happened.
        require!(
            !address_eq(&new_leader, &old_leader),
            DropsetError::SameLeader
        );
        // One open vault per leader per market, as at `create_vault`. Run
        // whatever state the target is in: rotating an unfrozen tombstone onto
        // a key that leads an open one creates no second open vault, but
        // the unconditional form keeps one rule for every path that seats
        // a leader.
        require!(
            !self.market.leads_open_vault(&new_leader),
            DropsetError::LeaderAlreadyLeadsVault
        );
        // The leader-and-depositor invariant: a vault's leader never holds
        // a `VaultDepositor` on that same vault. `deposit` rejects the
        // leader's signer and a fresh sector has no depositor PDAs, so this
        // is the third and last way in. Ownership, not lamports, is the
        // test — anyone can send lamports to the PDA address, and a dusted
        // address must not block a rotation. A closed PDA is reassigned to
        // the system program, so it reads as uninitialized here.
        require!(
            !self.new_leader_depositor.account().owned_by(&crate::ID),
            DropsetError::NewLeaderIsDepositor
        );

        self.market.mutate_vault(vault_idx)?.leader = new_leader;

        Ok(SetLeaderEvent {
            market: *self.market.address(),
            sector_idx: vault_idx,
            vault_seq,
            old_leader,
            new_leader,
            quote_authority,
        })
    }
}
