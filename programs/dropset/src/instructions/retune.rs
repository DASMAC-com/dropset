//! Post-create admin retuning levers.
//!
//! `create_vault` / `create_market` stamp a vault's `min_leader_share`
//! and a market's `fee_config` once, from the cascading defaults
//! (`Registry` → `MarketHeader` → `Vault`). These two admin-only
//! instructions retune those values afterward without re-creating the
//! account (architecture spec § SetMinLeaderShare, § SetMarketFeeConfig):
//!
//! * `set_min_leader_share` — rewrite one vault's skin-in-the-game floor
//!   (ppm). Pairs with `set_outside_deposits_approved`: the same admin
//!   sign-off that opens a vault to outside baskets can relax its floor.
//! * `set_market_fee_config` — rewrite a market's per-`CreateVault` fee
//!   (mint + owning token program + atoms). Takes effect on the next
//!   `CreateVault`; vaults already open are unaffected.
//!
//! Both authorize through the registry admin set — the same gate as
//! `set_outside_deposits_approved` / `freeze_vault` — and emit a
//! cold-path event so indexers can track the change. The
//! `SetMarketFeeConfig` event is load-bearing: the teardown fee sweep
//! reconstructs the set of historical fee mints from it (see the spec's
//! **Account lifecycle and rent reclamation**).

use anchor_lang_v2::{address_eq, prelude::*};
// `mint` stays in scope so the `mint::token_program` constraint on
// `fee_mint` expands to `anchor_spl_v2::mint::TokenProgramConstraint`.
#[allow(unused_imports)]
use anchor_spl_v2::{
    mint,
    token_interface::{Mint, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::{SetMarketFeeConfigEvent, SetMinLeaderShareEvent},
    state::Market,
    AdminSet, FeeConfig, Registry, PPM,
};

#[event_cpi]
#[derive(Accounts)]
pub struct SetMinLeaderShare {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market holding the target vault. `mut` for the floor write.
    #[account(mut)]
    pub market: Market,
}

impl SetMinLeaderShare {
    /// Returns the [`SetMinLeaderShareEvent`] payload for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn set_min_leader_share(
        &mut self,
        vault_idx: u32,
        min_leader_share: u32,
    ) -> Result<SetMinLeaderShareEvent> {
        // Admin-only — same gate as `set_outside_deposits_approved`. The
        // registry account is PDA-pinned (`seeds = [b"registry"]`), so
        // membership is checked against the canonical admin set.
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );
        // The floor is a fraction of total shares, so a value above 100%
        // (`PPM`) is unsatisfiable — reject it rather than silently brick
        // every future deposit on the vault. A floor of exactly `PPM` is
        // allowed: it pins the vault to a leader-only book (every outside
        // deposit fails the gate), which is a legitimate admin choice.
        require!(
            (min_leader_share as u64) <= PPM,
            DropsetError::InvalidMinLeaderShare
        );
        let len = self.market.len();
        require!((vault_idx as usize) < len, DropsetError::InvalidSectorIndex);

        let market_addr = *self.market.address();
        // Validate the sector is live through an immutable borrow before
        // taking the mutable one — the house pattern shared with the
        // other vault setters (`set_outside_deposits_approved`,
        // `freeze_vault`). A free-list sector carries `leader == default`.
        {
            let vault = &self.market.as_slice()[vault_idx as usize];
            require!(
                !address_eq(&vault.leader, &Address::default()),
                DropsetError::VaultEmpty
            );
        }
        self.market.as_mut_slice()[vault_idx as usize].min_leader_share = min_leader_share.into();

        Ok(SetMinLeaderShareEvent {
            market: market_addr,
            sector_idx: vault_idx,
            min_leader_share,
        })
    }
}

#[event_cpi]
#[derive(Accounts)]
pub struct SetMarketFeeConfig {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market whose `fee_config` is being retuned. `mut` for the write.
    #[account(mut)]
    pub market: Market,
    /// New fee mint. `mint::token_program = fee_token_program` ties it to
    /// the supplied program, so the stored `(mint, token_program)` pair
    /// is always a mint backed by its real owner — the check the spec
    /// states as `token_program == mint.owner`. `InterfaceAccount<Mint>`
    /// additionally rejects a non-mint payload.
    #[account(mint::token_program = fee_token_program)]
    pub fee_mint: InterfaceAccount<Mint>,
    /// Token program owning `fee_mint` — SPL Token or Token-2022.
    /// `Interface<TokenInterface>` rejects any non-token-program address
    /// up front; the `mint::token_program` constraint above then pins it
    /// to `fee_mint`'s actual owner.
    pub fee_token_program: Interface<'static, TokenInterface>,
}

impl SetMarketFeeConfig {
    /// Returns the [`SetMarketFeeConfigEvent`] payload for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn set_market_fee_config(&mut self, atoms: u64) -> Result<SetMarketFeeConfigEvent> {
        require!(
            self.registry.admin_contains(self.admin.address()),
            DropsetError::Unauthorized
        );

        // Read the validated mint/program before the mutable header
        // borrow; the `mint::token_program` constraint already proved the
        // pair is consistent, so the token program is not re-derived here.
        let mint = *self.fee_mint.address();
        let token_program = *self.fee_token_program.address();
        let market_addr = *self.market.address();

        self.market.fee_config = FeeConfig {
            mint,
            token_program,
            atoms: atoms.into(),
        };

        Ok(SetMarketFeeConfigEvent {
            market: market_addr,
            mint,
            token_program,
            atoms,
        })
    }
}
