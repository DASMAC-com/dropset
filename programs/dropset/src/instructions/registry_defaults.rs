//! Registry-default retuning lever.
//!
//! The `Registry` holds the protocol-wide defaults stamped onto *future*
//! markets at `create_market` (`default_taker_fee`,
//! `default_min_leader_share`, `default_fee_config`). Before this lever
//! they were write-once at `init`, so an operator could not adjust the
//! fee/floor schedule new markets inherit after launch.
//!
//! `set_registry_defaults` retunes the two scalar defaults
//! (`default_taker_fee`, `default_min_leader_share`) — each supplied as an
//! `Option`, so a caller can move one field without restating the others.
//! A `None` leaves that default untouched. The write is **non-retroactive**:
//! it changes only what the *next* `create_market` stamps, mirroring how
//! `SetMarketFeeConfig` takes effect only on the next `CreateVault`. Live
//! markets keep the values they were created with — retune those per market
//! via `set_taker_fee` / `set_min_leader_share`.
//!
//! `default_fee_config` is the registry's third default and is deliberately
//! *not* handled here: like `SetMarketFeeConfig`, mutating a fee config
//! must eagerly create the registry fee ATA for a new mint, so it belongs
//! in its own ATA-bearing instruction rather than as an `Option` field on a
//! pure-header writer. See the architecture spec, § SetRegistryDefaults.

use anchor_lang_v2::prelude::*;

use crate::{
    errors::DropsetError, events::SetRegistryDefaultsEvent, AdminSet, Registry, PPM,
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetRegistryDefaults {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry. `mut` for the default writes; also read for
    /// the admin-membership check.
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
}

impl SetRegistryDefaults {
    /// Apply the supplied (optional) registry-default updates and return
    /// the [`SetRegistryDefaultsEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`. Each `None` leaves its default unchanged.
    #[inline(always)]
    pub fn set_registry_defaults(
        &mut self,
        taker_fee: Option<u16>,
        min_leader_share: Option<u32>,
    ) -> Result<SetRegistryDefaultsEvent> {
        // Admin-only — same gate as the per-market retuning levers. The
        // registry is PDA-pinned (`seeds = [b"registry"]`), so membership
        // is checked against the canonical admin set.
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );

        // `default_taker_fee` is a `u16` (`Ppm16`), so the spec's ~6.55%
        // ceiling is the type bound — no range check is possible to fail.
        if let Some(taker_fee) = taker_fee {
            self.registry.default_taker_fee = taker_fee.into();
        }

        // The floor is a fraction of total shares, so a value above 100%
        // (`PPM`) is unsatisfiable — reject it rather than stamp an
        // out-of-range default onto every future vault. Exactly `PPM`
        // (a leader-only book) is allowed, matching `set_min_leader_share`.
        if let Some(min_leader_share) = min_leader_share {
            require!(
                (min_leader_share as u64) <= PPM,
                DropsetError::InvalidMinLeaderShare
            );
            self.registry.default_min_leader_share = min_leader_share.into();
        }

        Ok(SetRegistryDefaultsEvent {
            default_taker_fee: self.registry.default_taker_fee.get(),
            default_min_leader_share: self.registry.default_min_leader_share.get(),
        })
    }
}
