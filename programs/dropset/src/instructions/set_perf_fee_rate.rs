//! `set_perf_fee_rate` — leader-only performance-fee retune.
//!
//! Rewrites a vault's `perf_fee_rate` after creation. Per the
//! architecture spec § SetPerfFeeRate the gate is **leader-only**, like
//! `set_allow_outside_depositors`: the rate is the leader's term of
//! business, and depositors who dislike a raise may leave at any time.
//!
//! The handler **realizes first**: it runs `realize_in_place` at the
//! *old* rate before storing the new one, so an excess accrued under the
//! old rate is minted at the old rate and never at the new. The
//! high-water mark is value-per-share, independent of the rate, so the
//! realize leaves it at `max(hwm, VPS)` and the new rate only ever
//! applies to gains above that mark. On a frozen or tombstoned vault the
//! realize is a no-op (the mark is pinned there anyway) and the rate is
//! still written. The leader can already pin the mark at any moment with
//! a dust `deposit_leader`, so a realizing setter grants no new power.
//!
//! Like the other leader setters it touches no reference price, ladder,
//! or `market.nonce`: there is no flush to arm.

use anchor_lang::{address_eq, prelude::*};

use crate::{
    errors::DropsetError,
    events::{RealizeEvent, SetPerfFeeRateEvent},
    state::{realize_in_place, Market, VaultAccess},
    PPM,
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetPerfFeeRate {
    /// The vault's leader — the only signer this setter accepts.
    pub signer: Signer,
    /// Market holding the target vault. `mut` for the realize and the
    /// rate write.
    #[account(mut)]
    pub market: Market,
}

impl SetPerfFeeRate {
    /// Returns `(Option<RealizeEvent>, SetPerfFeeRateEvent)` for `lib.rs`
    /// to dispatch through `emit_cpi!`, the realize record first.
    #[inline(always)]
    pub fn set_perf_fee_rate(
        &mut self,
        vault_idx: u32,
        rate_ppm: u32,
    ) -> Result<(Option<RealizeEvent>, SetPerfFeeRateEvent)> {
        // The same bound `create_vault` applies to the initial rate.
        require!((rate_ppm as u64) <= PPM, DropsetError::InvalidPerfFeeRate);

        // Validate through an immutable borrow before taking the mutable
        // one — the house validate-then-mutate shape.
        let signer_addr = *self.signer.address();
        let (vault_seq, old_rate) = {
            let vault = self.market.read_vault(vault_idx)?;
            // A free-list sector carries `leader == default`; reject it
            // first so the error names the real cause rather than
            // surfacing as an authorization failure.
            require!(vault.is_occupied(), DropsetError::VaultEmpty);
            // Leader-only, never the `quote_authority`: the fee is a
            // custody-side term, not a quoting decision.
            require!(
                address_eq(&vault.leader, &signer_addr),
                DropsetError::Unauthorized
            );
            // A no-op retune would emit an event claiming a change that
            // never happened — rejected, as `set_quote_authority` rejects
            // re-stamping the incumbent.
            require!(
                vault.perf_fee_rate.get() != rate_ppm,
                DropsetError::SamePerfFeeRate
            );
            (vault.seq.get(), vault.perf_fee_rate.get())
        };

        // Realize at the old rate, then store the new one: the realize reads
        // `perf_fee_rate` before the store, so it always mints at the old
        // rate.
        let (outcome, leader_shares_after, total_shares_after) = {
            let vault = self.market.mutate_vault(vault_idx)?;
            let outcome = realize_in_place(vault);
            vault.perf_fee_rate = rate_ppm.into();
            (outcome, vault.leader_shares.get(), vault.total_shares.get())
        };

        let market = *self.market.address();
        let realize_event = RealizeEvent::from_outcome(
            &outcome,
            market,
            vault_idx,
            vault_seq,
            leader_shares_after,
            total_shares_after,
        );
        Ok((
            realize_event,
            SetPerfFeeRateEvent {
                market,
                sector_idx: vault_idx,
                vault_seq,
                leader: signer_addr,
                old_rate,
                new_rate: rate_ppm,
            },
        ))
    }
}
