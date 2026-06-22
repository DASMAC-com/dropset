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
//! `default_fee_config` is the registry's third default. Like
//! `SetMarketFeeConfig`, mutating a fee config must eagerly create the
//! registry fee ATA for the new mint, so it does *not* fold into the
//! `Option`-field, pure-header `set_registry_defaults` writer above — it
//! lives in its own ATA-bearing `set_default_fee_config` instruction
//! (below), the registry-level mirror of the per-market `SetMarketFeeConfig`.
//! See the architecture spec, § SetRegistryDefaults and § SetDefaultFeeConfig.

use anchor_lang_v2::prelude::*;
// `mint` / `associated_token` stay in scope so the `mint::token_program`
// constraint on `fee_mint` and the `associated_token::*` constraints on
// `registry_fee_treasury` expand to their `anchor_spl_v2` markers.
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::{self, AssociatedToken},
    mint,
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{SetDefaultFeeConfigEvent, SetRegistryDefaultsEvent},
    FeeConfig, Registry, PPM,
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
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
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

#[event_cpi]
#[derive(Accounts)]
pub struct SetDefaultFeeConfig {
    /// Registry admin — the only signer this lever accepts. `mut`
    /// because it funds the rent for the new mint's registry fee ATA
    /// when `registry_fee_treasury` is created below.
    #[account(mut)]
    pub admin: Signer,
    /// Singleton registry. `mut` to overwrite `default_fee_config`; also
    /// read for the admin-membership check and as the authority of the
    /// fee ATA created below.
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// New default fee mint. `mint::token_program = fee_token_program`
    /// ties it to the supplied program, so the stored
    /// `(mint, token_program)` pair is always a mint backed by its real
    /// owner — the check the spec states as `token_program == mint.owner`.
    /// `InterfaceAccount<Mint>` additionally rejects a non-mint payload.
    #[account(mint::token_program = fee_token_program)]
    pub fee_mint: InterfaceAccount<Mint>,
    /// Token program owning `fee_mint` — SPL Token or Token-2022.
    /// `Interface<TokenInterface>` rejects any non-token-program address
    /// up front; the `mint::token_program` constraint above then pins it
    /// to `fee_mint`'s actual owner.
    pub fee_token_program: Interface<'static, TokenInterface>,
    /// Registry's fee ATA for the new default mint. `create_market` loads
    /// the registry fee ATA for `registry.default_fee_config.mint` (`mut`,
    /// not `init`) but never creates it, so we create it here, eagerly, at
    /// config time — `init_if_needed` so re-pointing the default back to a
    /// mint whose ATA already exists is a no-op. The ATA program's
    /// `InitializeAccount3` CPI rejects a non-mint / wrong-program payload
    /// outright, a stronger backstop than the `mint::token_program`
    /// constraint above. Without this, switching the default to a fresh
    /// mint would brick the next `create_market` against it until the ATA
    /// was created out-of-band — the same hazard
    /// `SetMarketFeeConfig` guards at the market level (architecture spec
    /// § SetDefaultFeeConfig).
    #[account(
        init_if_needed,
        payer = admin,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = fee_token_program,
    )]
    pub registry_fee_treasury: InterfaceAccount<TokenAccount>,
    pub associated_token_program: Program<AssociatedToken>,
    pub system_program: Program<System>,
}

impl SetDefaultFeeConfig {
    /// Returns the [`SetDefaultFeeConfigEvent`] payload for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn set_default_fee_config(&mut self, atoms: u64) -> Result<SetDefaultFeeConfigEvent> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        // Read the validated mint/program before the mutable registry
        // borrow; the `mint::token_program` constraint already proved the
        // pair is consistent, so the token program is not re-derived here.
        let mint = *self.fee_mint.address();
        let token_program = *self.fee_token_program.address();

        self.registry.default_fee_config = FeeConfig {
            mint,
            token_program,
            atoms: atoms.into(),
        };

        Ok(SetDefaultFeeConfigEvent {
            mint,
            token_program,
            atoms,
        })
    }
}
