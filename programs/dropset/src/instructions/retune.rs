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
//!   `CreateVault`; vaults already open are unaffected. Eagerly creates
//!   the registry's fee ATA for the new mint (`init_if_needed`, admin
//!   pays rent) so the fee destination provably exists the moment the
//!   config is set — `CreateVault` loads that ATA but never creates it.
//! * `set_taker_fee` — rewrite a market's taker fee (ppm, [`Ppm16`]),
//!   read on the swap hot path. A market-wide knob, not per-vault, so it
//!   takes no `vault_idx`. `Ppm16` is a `u16`, so the spec's "~6.55%"
//!   cap is the type bound — no value can exceed it and no range check
//!   is needed.
//!
//! Both authorize through the registry admin set — the same gate as
//! `set_outside_deposits_approved` / `freeze_vault` — and emit a
//! cold-path event so indexers can track the change. The
//! `SetMarketFeeConfig` event is load-bearing: the teardown fee sweep
//! reconstructs the set of historical fee mints from it (see the spec's
//! **Account lifecycle and rent reclamation**).

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
    events::{SetMarketFeeConfigEvent, SetMinLeaderShareEvent, SetTakerFeeEvent},
    state::{Market, VaultAccess},
    FeeConfig, Registry, PPM,
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
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        // The floor is a fraction of total shares, so a value above 100%
        // (`PPM`) is unsatisfiable — reject it rather than silently brick
        // every future deposit on the vault. A floor of exactly `PPM` is
        // allowed: it pins the vault to a leader-only book (every outside
        // deposit fails the gate), which is a legitimate admin choice.
        require!(
            (min_leader_share as u64) <= PPM,
            DropsetError::InvalidMinLeaderShare
        );
        let market_addr = *self.market.address();
        // Validate the sector is live through an immutable borrow before
        // taking the mutable one — the house pattern shared with the
        // other vault setters (`set_outside_deposits_approved`,
        // `freeze_vault`). `is_occupied` is the free-list marker check.
        require!(
            self.market.read_vault(vault_idx)?.is_occupied(),
            DropsetError::VaultEmpty
        );
        self.market.mutate_vault(vault_idx)?.min_leader_share = min_leader_share.into();

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
    /// Registry admin — the only signer this lever accepts. `mut`
    /// because it funds the rent for the new mint's registry fee ATA
    /// when `registry_fee_treasury` is created below.
    #[account(mut)]
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check and as
    /// the authority of the fee ATA created below.
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
    /// Registry's fee ATA for the new mint. `CreateVault` charges the
    /// per-vault fee into this account but does **not** create it, so we
    /// create it here, eagerly, at config time — `init_if_needed` so
    /// re-pointing a market back to a mint whose ATA already exists is a
    /// no-op. The ATA program's `InitializeAccount3` CPI rejects a
    /// non-mint / wrong-program payload outright, a stronger backstop than
    /// the `mint::token_program` constraint above. Without this, switching
    /// a market to a fresh mint would brick the next `CreateVault` on it
    /// until the ATA was created out-of-band (architecture spec
    /// § SetMarketFeeConfig).
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

impl SetMarketFeeConfig {
    /// Returns the [`SetMarketFeeConfigEvent`] payload for `lib.rs` to
    /// dispatch through `emit_cpi!`.
    #[inline(always)]
    pub fn set_market_fee_config(&mut self, atoms: u64) -> Result<SetMarketFeeConfigEvent> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
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

#[event_cpi]
#[derive(Accounts)]
pub struct SetTakerFee {
    /// Registry admin — the only signer this lever accepts.
    pub admin: Signer,
    /// Singleton registry, read for the admin-membership check.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,
    /// Market whose `taker_fee` is being retuned. `mut` for the write.
    #[account(mut)]
    pub market: Market,
}

impl SetTakerFee {
    /// Returns the [`SetTakerFeeEvent`] payload for `lib.rs` to dispatch
    /// through `emit_cpi!`.
    #[inline(always)]
    pub fn set_taker_fee(&mut self, taker_fee: u16) -> Result<SetTakerFeeEvent> {
        // Admin-only — gated at the dispatcher via `#[access_control]`
        // (`lib.rs`), so the caller is already a known admin here.
        // No range check: `taker_fee` is a `u16` (`Ppm16`), so the spec's
        // ~6.55% ceiling is the type's own max — unreachable to exceed.
        let market_addr = *self.market.address();
        self.market.taker_fee = taker_fee.into();

        Ok(SetTakerFeeEvent {
            market: market_addr,
            taker_fee,
        })
    }
}
