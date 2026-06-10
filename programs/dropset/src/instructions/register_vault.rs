//! `register_vault` (spec's `OpenVault`) — allocate a new vault sector
//! and stamp it with the leader's parameters.
//!
//! Charges `market.fee_config.atoms` of `market.fee_config.mint` to the
//! Registry's fee ATA — waived when the signer is a registry admin.
//! Allocates a sector via [`crate::state::VaultDll::allocate_sector`]
//! (free list reuse, else slab realloc), threads it onto the active
//! DLL, and writes the leader's pubkey, quote authority, perf-fee rate,
//! `min_leader_share` (stamped from the market default), and HWM seed.
//!
//! Admins may pass a `leader_override` to seat a vault on someone
//! else's behalf — useful for issuer-funded vaults where the protocol
//! seeds a market maker. Non-admin callers must pass the
//! [`Address::default()`] sentinel (or their own pubkey) — any other
//! value is rejected with [`DropsetError::LeaderOverrideNotAllowed`].

use anchor_lang_v2::{address_eq, prelude::*};
// `associated_token::{self, ...}` keeps the module in scope so the
// `associated_token::*` constraint paths on `registry_fee_treasury`
// expand to `anchor_spl_v2::associated_token::<Marker>`.
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::{self, AssociatedToken},
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    events::OpenVaultEvent,
    state::{DllList, Market, VaultDll},
    AdminSet, Registry, PPM, Q32_32_ONE,
};

#[event_cpi]
#[derive(Accounts)]
pub struct RegisterVault {
    /// Pays sector-rent top-up (if the slab realloc grows the
    /// account) and the open-vault fee (unless waived for an admin).
    /// Becomes the vault's `leader` unless an admin supplied a
    /// distinct `leader_override` — see the handler.
    #[account(mut)]
    pub payer: Signer,

    /// Singleton registry — read for admin check + the
    /// `max_vaults_per_market` cap.
    #[account(seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,

    /// Market the vault lives on. `mut` because the slab tail grows
    /// (or pops from the free list) and `active_count` increments.
    #[account(mut)]
    pub market: Market,

    /// Mint the open-vault fee is paid in. Pinned to the value
    /// `register_market` stamped into `market.fee_config.mint`.
    #[account(address = market.fee_config.mint)]
    pub fee_mint: InterfaceAccount<Mint>,
    /// Token program owning `fee_mint`. Pinned to the value stamped at
    /// market creation.
    #[account(address = market.fee_config.token_program)]
    pub fee_token_program: Interface<'static, TokenInterface>,
    /// Caller's source ATA for the fee mint. Only read on the
    /// non-admin path; admin path skips the transfer entirely.
    #[account(mut)]
    pub payer_fee_source: UncheckedAccount,
    /// Registry's fee ATA. ATA constraint validates
    /// `(registry, fee_mint, fee_token_program)` so a wrong destination
    /// is rejected before the transfer CPI runs.
    #[account(
        mut,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = fee_token_program,
    )]
    pub registry_fee_treasury: InterfaceAccount<TokenAccount>,

    pub system_program: Program<System>,
}

impl RegisterVault {
    /// Run the handler body and return the [`OpenVaultEvent`] payload
    /// for `lib.rs` to dispatch through `emit_cpi!`. The macro
    /// requires `ctx` in scope, which the `impl` method can't see —
    /// keeping the emit out here means the spec's "events ride as
    /// inner-instruction data, not logs" rule (§ Events and emission)
    /// holds without restructuring every handler to take `ctx`.
    #[inline(always)]
    pub fn register_vault(
        &mut self,
        perf_fee_rate: u32,
        quote_authority: Address,
        allow_outside_depositors: bool,
        leader_override: Address,
    ) -> Result<OpenVaultEvent> {
        // Validate perf fee. Capped at 100% (`PPM`). The spec leaves
        // this open-ended; the cap matches the `Ppm32` semantic.
        require!(
            (perf_fee_rate as u64) <= PPM,
            DropsetError::InvalidPerfFeeRate
        );
        // Reject `Address::default()` — the zero pubkey is the
        // free-list emptiness marker; if a leader stamped it as
        // `quote_authority` the vault would be quote-bricked
        // (`set_reference_price` checks `signer == quote_authority`
        // and the zero address has no private key). Use the leader
        // pubkey when the caller wants "no separate delegation".
        require!(
            !address_eq(&quote_authority, &Address::default()),
            DropsetError::Unauthorized
        );

        // Cap check before doing any work.
        let max_vaults = self.registry.max_vaults_per_market as u32;
        let active = self.market.active_count.get();
        require!(active < max_vaults, DropsetError::VaultCapExceeded);

        // Resolve the leader. Spec § OpenVault:
        // - Non-admin caller: must pass `Address::default()` (no
        //   override) or their own pubkey — anything else is a
        //   misuse of the admin-only override.
        // - Admin caller: may pass any pubkey; that pubkey becomes
        //   `Vault.leader`. `Address::default()` means "use payer".
        let payer_addr = *self.payer.address();
        let is_admin = self.registry.admin_contains(&payer_addr);
        let override_used = !address_eq(&leader_override, &Address::default());
        if !is_admin && override_used {
            require!(
                address_eq(&leader_override, &payer_addr),
                DropsetError::LeaderOverrideNotAllowed
            );
        }
        let leader = if override_used {
            leader_override
        } else {
            payer_addr
        };

        // Charge the open-vault fee unless the signer is an admin.
        if !is_admin {
            let atoms = self.market.fee_config.atoms.get();
            if atoms > 0 {
                let decimals = self.fee_mint.decimals();
                let cpi = CpiContext::new(
                    self.fee_token_program.address(),
                    TransferChecked {
                        from: self.payer_fee_source.cpi_handle_mut(),
                        mint: self.fee_mint.cpi_handle(),
                        to: self.registry_fee_treasury.cpi_handle_mut(),
                        authority: self.payer.cpi_handle(),
                    },
                );
                transfer_checked(cpi, atoms, decimals)?;
            }
        }

        // Allocate a sector — reuses a free-list entry when available,
        // else extends the slab. Tops up any rent shortfall from
        // `payer`.
        let sector = self.market.allocate_sector(self.payer.as_ref())?;
        self.market.link_head(DllList::Active, sector)?;
        self.market.active_count = (active + 1).into();

        // Stamp the new sector. `allocate_sector` zeroed it, so we
        // only need to write the leader-controlled fields.
        let market_addr = *self.market.address();
        let min_leader_share = self.market.default_min_leader_share.get();
        let vault = &mut self.market.as_mut_slice()[sector as usize];
        vault.leader = leader;
        vault.quote_authority = quote_authority;
        vault.perf_fee_rate = perf_fee_rate.into();
        vault.min_leader_share = min_leader_share.into();
        vault.hwm = Q32_32_ONE.into();
        vault.allow_outside_depositors = allow_outside_depositors.into();
        // `frozen`, `outside_deposits_approved`, base/quote/share
        // counters, profile, and remaining are already zero from
        // `allocate_sector`'s `Vault::zeroed()`.

        Ok(OpenVaultEvent {
            market: market_addr,
            sector_idx: sector,
            leader,
            quote_authority,
            perf_fee_rate,
            min_leader_share,
            allow_outside_depositors,
        })
    }
}
