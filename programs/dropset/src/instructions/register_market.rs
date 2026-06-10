//! `register_market` — bring a new (base, quote) pair online.
//!
//! Allocates the `MarketHeader` PDA seeded by `(base_mint, quote_mint)`,
//! creates the base and quote treasury ATAs (owned by the market PDA),
//! and (unless the caller is an admin) transfers the registry's
//! open-market fee from the caller's source ATA to the registry's fee
//! vault. The slab tail starts at zero capacity — vaults are added
//! separately by `OpenVault`.
//!
//! The registry fee vault was created at `init` time, so this
//! instruction only validates that the supplied destination is the
//! canonical ATA and transfers into it. The mints themselves are
//! validated as actual `Mint` accounts under either SPL Token or
//! Token-2022 by `InterfaceAccount<Mint>`; the ATA program then
//! refuses to initialize a treasury against a non-mint, so no extra
//! mint plumbing is needed in this handler.

use anchor_lang_v2::{address_eq, find_and_verify_program_address, prelude::*};
// `associated_token::{self, ...}` keeps the module in scope so the
// derive can expand `associated_token::*` constraint expressions to
// `anchor_spl_v2::associated_token::<Marker>` paths.
#[allow(unused_imports)]
use anchor_spl_v2::{
    associated_token::{self, AssociatedToken},
    token_2022::{transfer_checked, TransferChecked},
    token_interface::{Mint, TokenAccount, TokenInterface},
};

use crate::{
    errors::DropsetError,
    state::{Market, NULL_SECTOR},
    AdminSet, FeeConfig, Registry,
};

/// Accounts for the `register_market` instruction: a fresh
/// `(base_mint, quote_mint)` market PDA plus its two treasury ATAs,
/// charged against the registry's per-`OpenVault` fee (waived for
/// admins). See the module doc for the spec-level intent.
#[derive(Accounts)]
pub struct RegisterMarket {
    /// Funds rent for the market PDA and the two treasury ATAs, and
    /// signs the fee transfer unless waived for an admin signer.
    #[account(mut)]
    pub payer: Signer,

    /// Singleton registry. Mutated to bump `market_count`, and read
    /// for the fee config, admin set, and stamped defaults.
    #[account(mut, seeds = [b"registry"], bump = registry.bump)]
    pub registry: Registry,

    /// Base leg mint. `InterfaceAccount<Mint>` accepts SPL Token and
    /// Token-2022 by relaxing the owner check; the deserialization
    /// rejects non-mint payloads (token accounts, multisigs, etc.).
    pub base_mint: InterfaceAccount<Mint>,
    /// Quote leg mint. Same deal as `base_mint`.
    pub quote_mint: InterfaceAccount<Mint>,

    /// SPL Token or Token-2022 — whichever owns `base_mint`.
    /// `Interface<TokenInterface>` rejects any other address up front
    /// with `IncorrectProgramId`; an actual `(mint, token_program)`
    /// owner mismatch surfaces later when the ATA program CPIs through
    /// to it.
    pub base_token_program: Interface<'static, TokenInterface>,
    /// Same for the quote leg.
    pub quote_token_program: Interface<'static, TokenInterface>,

    /// Market PDA seeded by `(base_mint, quote_mint)`. The slab tail
    /// holds the vault sectors; we open with zero capacity here and
    /// let `OpenVault` grow it. The `init` constraint enforces
    /// single-shot creation — a second `register_market` against the
    /// same pair is rejected by the runtime before our handler runs.
    #[account(
        init,
        payer = payer,
        space = Market::space_for(0),
        seeds = [base_mint.address().as_ref(), quote_mint.address().as_ref()],
        bump,
    )]
    pub market: Market,

    /// Pooled base inventory for every vault on this market. The ATA
    /// program is the only path that creates it, and its
    /// `InitializeAccount3` CPI to the token program is what enforces
    /// `base_mint` is a real mint under the supplied token program.
    #[account(
        init,
        payer = payer,
        associated_token::mint = base_mint,
        associated_token::authority = market,
        associated_token::token_program = base_token_program,
    )]
    pub base_treasury: InterfaceAccount<TokenAccount>,
    /// Pooled quote inventory. See `base_treasury`.
    #[account(
        init,
        payer = payer,
        associated_token::mint = quote_mint,
        associated_token::authority = market,
        associated_token::token_program = quote_token_program,
    )]
    pub quote_treasury: InterfaceAccount<TokenAccount>,

    /// Mint the open-market fee is charged in. The `address` constraint
    /// binds it to whatever was stamped onto the registry at `init`,
    /// so a wrong mint here yields `ConstraintAddress` before the
    /// handler runs.
    #[account(address = registry.default_fee_config.mint)]
    pub fee_mint: InterfaceAccount<Mint>,
    /// Token program that owns `fee_mint`. Pinned to the value the
    /// registry stamped at `init`, so a caller-supplied mismatch is
    /// rejected up front. The `Interface<TokenInterface>` bound is
    /// belt-and-braces — the address check already implies it.
    #[account(address = registry.default_fee_config.token_program)]
    pub fee_token_program: Interface<'static, TokenInterface>,
    /// Caller's source ATA holding the fee mint. Only read on the
    /// non-admin path — admins skip the transfer entirely, so the
    /// unchecked passthrough is safe: any account here is ignored on
    /// the admin branch.
    #[account(mut)]
    pub payer_fee_source: UncheckedAccount,
    /// Registry's fee vault — created at `init` time. The
    /// `associated_token::*` constraints re-validate it against
    /// `(registry, fee_mint, fee_token_program)` so a non-canonical
    /// destination is rejected by `ConstraintAddress`.
    #[account(
        mut,
        associated_token::mint = fee_mint,
        associated_token::authority = registry,
        associated_token::token_program = fee_token_program,
    )]
    pub registry_fee_treasury: InterfaceAccount<TokenAccount>,

    pub system_program: Program<System>,
    pub associated_token_program: Program<AssociatedToken>,
}

impl RegisterMarket {
    #[inline(always)]
    pub fn register_market(&mut self, bump: u8) -> Result<()> {
        // Reject same-mint markets — the PDA would still derive, but a
        // single-mint book is meaningless.
        require!(
            !address_eq(self.base_mint.address(), self.quote_mint.address()),
            DropsetError::DuplicateBaseQuoteMint
        );

        // Charge the fee unless the caller is a registry admin.
        if !self.registry.admin_contains(self.payer.address()) {
            let atoms = self.registry.default_fee_config.atoms.get();
            // A zero fee is a config choice (e.g. testing). Skip the
            // CPI rather than emit a no-op transfer.
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

        // The ATA bumps aren't tracked in `ctx.bumps` (the framework
        // only records seed-derived bumps for `seeds = …` accounts on
        // this struct). Recover them via
        // `find_and_verify_program_address`, which short-circuits once
        // it matches the already-created ATA.
        let ata_program = anchor_lang_v2::programs::AssociatedToken::id();
        let base_treasury_bump = find_and_verify_program_address(
            &[
                self.market.address().as_ref(),
                self.base_token_program.address().as_ref(),
                self.base_mint.address().as_ref(),
            ],
            &ata_program,
            self.base_treasury.address(),
        )?;
        let quote_treasury_bump = find_and_verify_program_address(
            &[
                self.market.address().as_ref(),
                self.quote_token_program.address().as_ref(),
                self.quote_mint.address().as_ref(),
            ],
            &ata_program,
            self.quote_treasury.address(),
        )?;

        // Stamp the header. `init` zeroed the slab, so list heads
        // start at `NULL_SECTOR` (`u32::MAX`); we set them explicitly
        // anyway so the invariant is local to this handler.
        // `active_count`, `outstanding_vault_depositors`, `nonce`, and
        // the slab tail's `len` field are already zero.
        let base_mint_addr = *self.base_mint.address();
        let quote_mint_addr = *self.quote_mint.address();
        let base_treasury_addr = *self.base_treasury.address();
        let quote_treasury_addr = *self.quote_treasury.address();
        let default_fee_config = self.registry.default_fee_config;
        let default_taker_fee = self.registry.default_taker_fee.get();
        let default_min_leader_share = self.registry.default_min_leader_share.get();

        let market = &mut self.market;
        market.bump = bump;
        market.base_treasury_bump = base_treasury_bump;
        market.quote_treasury_bump = quote_treasury_bump;
        market.head = NULL_SECTOR.into();
        market.tombstone_head = NULL_SECTOR.into();
        market.free_head = NULL_SECTOR.into();
        market.active_count = 0u32.into();
        market.outstanding_vault_depositors = 0u32.into();
        market.nonce = 0u64.into();
        market.base_mint = base_mint_addr;
        market.quote_mint = quote_mint_addr;
        market.base_treasury = base_treasury_addr;
        market.quote_treasury = quote_treasury_addr;
        market.taker_fee = default_taker_fee.into();
        market.default_min_leader_share = default_min_leader_share.into();
        market.fee_config = FeeConfig {
            mint: default_fee_config.mint,
            token_program: default_fee_config.token_program,
            atoms: default_fee_config.atoms.get().into(),
        };

        // Bump the registry's live-market counter — the on-chain
        // witness `close_registry` checks against under the
        // `admin-teardown` feature. See architecture spec,
        // **Account lifecycle and rent reclamation**.
        let prev = self.registry.market_count.get();
        self.registry.market_count = prev
            .checked_add(1)
            .ok_or(DropsetError::MarketCountOverflow)?
            .into();
        Ok(())
    }
}
