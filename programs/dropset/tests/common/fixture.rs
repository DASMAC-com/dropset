//! Shared market-bootstrap fixture and per-instruction ix-builders.
//!
//! Every integration test that needs a live market repeated the same
//! `init → register_market → register_vault → set_reference_price →
//! set_liquidity_profile → seed` plumbing (150-300 lines apiece). This
//! module collapses that to a handful of calls on a [`Fixture`] that
//! owns the `LiteSVM` and every derived handle, plus thin ix-builders
//! so a test reads as intent ("deposit, then withdraw half") rather
//! than `AccountMeta` lists.
//!
//! Production handlers can't yet set some fields a negative test needs
//! (`min_leader_share`, `taker_fee`). The `poke_*` helpers rewrite those
//! fields directly on the account and reinstall it. Each `poke_*` call
//! site should be replaced with the real admin instruction once it
//! lands; they exist only to reach states the on-chain code can't yet
//! produce on its own. (`frozen` graduated to the real `freeze_vault`
//! instruction — see [`Fixture::freeze_vault`].)

#![allow(dead_code)]

use super::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, deploy_with_authority, mint_to, send_ixn, ATA_PROGRAM_ID, PROGRAM_ID,
    REGISTER_MARKET_FEE_ATOMS, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID,
};
use anchor_lang_v2::{bytemuck, programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, LiteSVM, Signer};
use dropset::{
    instruction::{
        CloseMarket as CloseMarketIx, CloseMarketTreasury as CloseMarketTreasuryIx,
        CloseRegistry as CloseRegistryIx, CloseRegistryFeeVault as CloseRegistryFeeVaultIx,
        CloseVault as CloseVaultIx, Deposit as DepositIx, DepositLeader as DepositLeaderIx,
        ForceWithdrawDepositor as ForceWithdrawDepositorIx,
        ForceWithdrawLeader as ForceWithdrawLeaderIx, FreezeVault as FreezeVaultIx, Init as InitIx,
        RegisterMarket as RegisterMarketIx, RegisterVault as RegisterVaultIx,
        SetAllowOutsideDepositors as SetAllowOutsideDepositorsIx,
        SetLiquidityProfile as SetLiquidityProfileIx,
        SetOutsideDepositsApproved as SetOutsideDepositsApprovedIx,
        SetReferencePrice as SetReferencePriceIx, Swap as SwapIx, Withdraw as WithdrawIx,
        WithdrawLeader as WithdrawLeaderIx,
    },
    LiquidityProfile, MarketHeader, Price, RegistryHeader, Vault, VaultDepositorHeader, N_LEVELS,
};
use solana_instruction::{AccountMeta, Instruction};
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::Pubkey;

pub const SYSVAR_CLOCK_ID: Pubkey =
    Pubkey::from_str_const("SysvarC1ock11111111111111111111111111111111");

/// On-wire size of a serialized [`LiquidityProfile`] (alignment-1 Pod).
pub const PROFILE_BYTES: usize = 2 * N_LEVELS * 10;

// ── PDA derivations ──────────────────────────────────────────────────

pub fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &PROGRAM_ID).0
}
pub fn market_pda(base: &Pubkey, quote: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[base.as_ref(), quote.as_ref()], &PROGRAM_ID).0
}
pub fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &PROGRAM_ID).0
}
pub fn vault_depositor_pda(market: &Pubkey, vault_idx: u32, owner: &Pubkey) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[
            b"vault_depositor",
            market.as_ref(),
            &vault_idx.to_le_bytes(),
            owner.as_ref(),
        ],
        &PROGRAM_ID,
    )
}

// ── Account-data layout helpers ──────────────────────────────────────

/// Byte offset of the first [`Vault`] sector inside a market account
/// (`8-byte disc + MarketHeader + 4-byte slab len`, then aligned to
/// `Vault`'s alignment — 1 for our alignment-1 layout).
fn vault_items_start() -> usize {
    let after_len = 8 + core::mem::size_of::<MarketHeader>() + 4;
    let align = core::mem::align_of::<Vault>();
    (after_len + align - 1) & !(align - 1)
}

fn vault_byte_offset(sector_idx: u32) -> usize {
    vault_items_start() + sector_idx as usize * core::mem::size_of::<Vault>()
}

/// Build a one-bid/one-ask profile: symmetric `offset_ppm` spread,
/// `size_bps` of each leg, never-expiring. The default ladder seeded
/// vaults quote with.
pub fn simple_profile(offset_ppm: u32, size_bps: u16, expiry_offset: u32) -> [u8; PROFILE_BYTES] {
    let mut profile: LiquidityProfile = bytemuck::Zeroable::zeroed();
    profile.bids[0].price_offset = offset_ppm.into();
    profile.bids[0].size_bps = size_bps.into();
    profile.bids[0].expiry_offset = expiry_offset.into();
    profile.asks[0].price_offset = offset_ppm.into();
    profile.asks[0].size_bps = size_bps.into();
    profile.asks[0].expiry_offset = expiry_offset.into();
    let mut bytes = [0u8; PROFILE_BYTES];
    bytes.copy_from_slice(bytemuck::bytes_of(&profile));
    bytes
}

// ── Fixture ──────────────────────────────────────────────────────────

/// A live market on a `LiteSVM`. `authority` is the genesis admin, the
/// default vault leader, and the default quote authority — most tests
/// only need this one key. Spin up extra signers with
/// [`Fixture::funded_keypair`] / [`Fixture::funded_depositor`].
pub struct Fixture {
    pub svm: LiteSVM,
    pub authority: Keypair,
    pub registry: Pubkey,
    pub fee_mint: Pubkey,
    pub registry_fee_treasury: Pubkey,
    /// Stand-in `payer_fee_source` for the admin register paths (the
    /// fee transfer is skipped for admins, so it is never read).
    pub dummy: Keypair,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub market: Pubkey,
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
}

impl Fixture {
    /// `init` the registry and `register_market` a fresh
    /// base/quote pair. No vault yet — call [`Self::register_vault`].
    pub fn bootstrap() -> Self {
        let authority = Keypair::new();
        let mut svm = deploy_with_authority(&authority);
        let fee_mint = create_mock_usdc_mint(&mut svm, &authority);
        let registry = registry_pda();
        let registry_fee_treasury =
            associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);

        // init.
        let init_ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &InitIx {
                genesis_admin: authority.pubkey(),
                fee_atoms: REGISTER_MARKET_FEE_ATOMS,
            }
            .data(),
            vec![
                AccountMeta::new(authority.pubkey(), true),
                AccountMeta::new(registry, false),
                AccountMeta::new_readonly(get_program_data_address(&PROGRAM_ID), false),
                AccountMeta::new_readonly(fee_mint, false),
                AccountMeta::new(registry_fee_treasury, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
            ],
        );
        send_ixn(&mut svm, &authority, init_ix).expect("init");

        // register_market.
        let base_mint = create_spl_mint(&mut svm, &authority);
        let quote_mint = create_spl_mint(&mut svm, &authority);
        let market = market_pda(&base_mint, &quote_mint);
        let base_treasury = associated_token_address(&market, &base_mint, &SPL_TOKEN_PROGRAM_ID);
        let quote_treasury = associated_token_address(&market, &quote_mint, &SPL_TOKEN_PROGRAM_ID);
        let dummy = Keypair::new();
        svm.airdrop(&dummy.pubkey(), SIGNER_FUNDING_LAMPORTS)
            .unwrap();
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &RegisterMarketIx {}.data(),
            vec![
                AccountMeta::new(authority.pubkey(), true),
                AccountMeta::new(registry, false),
                AccountMeta::new_readonly(base_mint, false),
                AccountMeta::new_readonly(quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(market, false),
                AccountMeta::new(base_treasury, false),
                AccountMeta::new(quote_treasury, false),
                AccountMeta::new_readonly(fee_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(dummy.pubkey(), false),
                AccountMeta::new(registry_fee_treasury, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut svm, &authority, ix).expect("register_market");

        Fixture {
            svm,
            authority,
            registry,
            fee_mint,
            registry_fee_treasury,
            dummy,
            base_mint,
            quote_mint,
            market,
            base_treasury,
            quote_treasury,
        }
    }

    /// Bootstrap + open one admin vault (sector 0) + set a 1.0850
    /// reference price + a full-inventory ±5_000 ppm ladder + seed it
    /// with `(base, quote)` via `deposit_leader`. The state every swap
    /// / withdraw test starts from. `quote_authority` and `leader` are
    /// both `authority`.
    pub fn seeded(base: u64, quote: u64) -> Self {
        let mut f = Self::bootstrap();
        f.register_vault(0, f.authority.pubkey(), false, Pubkey::default())
            .expect("register_vault");
        let ref_price = Price::encode(10_850_000, 0).unwrap();
        f.set_reference_price(&f.authority.insecure_clone(), 0, ref_price.as_u32(), 0)
            .expect("set_reference_price");
        f.set_liquidity_profile(
            &f.authority.insecure_clone(),
            0,
            simple_profile(5_000, 10_000, u32::MAX),
        )
        .expect("set_liquidity_profile");
        f.deposit_leader(0, base, quote, base, quote)
            .expect("seed deposit_leader");
        f
    }

    // ── signer / ATA helpers ─────────────────────────────────────────

    pub fn funded_keypair(&mut self, lamports: u64) -> Keypair {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), lamports).unwrap();
        kp
    }

    /// A new keypair with funded ATAs holding `base` / `quote` atoms.
    pub fn funded_depositor(&mut self, base: u64, quote: u64) -> Keypair {
        let kp = self.funded_keypair(10 * SIGNER_FUNDING_LAMPORTS);
        let (b_ata, q_ata) = self.create_atas(&kp.pubkey());
        let auth = self.authority.insecure_clone();
        if base > 0 {
            mint_to(&mut self.svm, &auth, &self.base_mint, &b_ata, base);
        }
        if quote > 0 {
            mint_to(&mut self.svm, &auth, &self.quote_mint, &q_ata, quote);
        }
        kp
    }

    /// Create (idempotently) the base+quote ATAs for `owner`, paid by
    /// `authority`. Returns `(base_ata, quote_ata)`. Safe to call
    /// repeatedly — the underlying ATA `Create` is not idempotent, so
    /// we skip a leg that already exists (e.g. a leader topping up
    /// after a seed).
    pub fn create_atas(&mut self, owner: &Pubkey) -> (Pubkey, Pubkey) {
        let auth = self.authority.insecure_clone();
        let b = self.base_ata(owner);
        let q = self.quote_ata(owner);
        if self.svm.get_account(&b).is_none() {
            create_associated_token_account(
                &mut self.svm,
                &auth,
                owner,
                &self.base_mint,
                &SPL_TOKEN_PROGRAM_ID,
            );
        }
        if self.svm.get_account(&q).is_none() {
            create_associated_token_account(
                &mut self.svm,
                &auth,
                owner,
                &self.quote_mint,
                &SPL_TOKEN_PROGRAM_ID,
            );
        }
        (b, q)
    }

    pub fn base_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token_address(owner, &self.base_mint, &SPL_TOKEN_PROGRAM_ID)
    }
    pub fn quote_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token_address(owner, &self.quote_mint, &SPL_TOKEN_PROGRAM_ID)
    }

    // ── instruction senders ──────────────────────────────────────────

    /// `register_vault` via the admin path (payer = `authority`, fee
    /// waived). Returns `Err(debug-string)` on program rejection.
    pub fn register_vault(
        &mut self,
        perf_fee_rate: u32,
        quote_authority: Pubkey,
        allow_outside_depositors: bool,
        leader_override: Pubkey,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &RegisterVaultIx {
                perf_fee_rate,
                quote_authority,
                allow_outside_depositors,
                leader_override,
            }
            .data(),
            vec![
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.fee_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(self.dummy.pubkey(), false),
                AccountMeta::new(self.registry_fee_treasury, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        let auth = self.authority.insecure_clone();
        send_ixn(&mut self.svm, &auth, ix)
    }

    /// `register_vault` via the **non-admin** fee path: `payer` signs
    /// and pays the per-market open-vault fee out of its fee-mint ATA
    /// (funded here with exactly the fee amount). Returns
    /// `Err(debug-string)` on program rejection.
    pub fn register_vault_as(
        &mut self,
        payer: &Keypair,
        perf_fee_rate: u32,
        quote_authority: Pubkey,
        allow_outside_depositors: bool,
        leader_override: Pubkey,
    ) -> Result<(), String> {
        let auth = self.authority.insecure_clone();
        let fee_src = self.fee_ata(&payer.pubkey());
        if self.svm.get_account(&fee_src).is_none() {
            create_associated_token_account(
                &mut self.svm,
                &auth,
                &payer.pubkey(),
                &self.fee_mint,
                &SPL_TOKEN_PROGRAM_ID,
            );
        }
        mint_to(
            &mut self.svm,
            &auth,
            &self.fee_mint,
            &fee_src,
            REGISTER_MARKET_FEE_ATOMS,
        );
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &RegisterVaultIx {
                perf_fee_rate,
                quote_authority,
                allow_outside_depositors,
                leader_override,
            }
            .data(),
            vec![
                AccountMeta::new(payer.pubkey(), true),
                AccountMeta::new(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.fee_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(fee_src, false),
                AccountMeta::new(self.registry_fee_treasury, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, payer, ix)
    }

    /// ATA holding the open-vault fee mint for `owner`.
    pub fn fee_ata(&self, owner: &Pubkey) -> Pubkey {
        associated_token_address(owner, &self.fee_mint, &SPL_TOKEN_PROGRAM_ID)
    }

    pub fn set_reference_price(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        price_bits: u32,
        quote_slot: u64,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetReferencePriceIx {
                vault_idx,
                price_bits,
                quote_slot,
            }
            .data(),
            vec![
                AccountMeta::new_readonly(signer.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    pub fn set_liquidity_profile(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        profile_bytes: [u8; PROFILE_BYTES],
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetLiquidityProfileIx {
                vault_idx,
                profile_bytes,
            }
            .data(),
            vec![
                AccountMeta::new_readonly(signer.pubkey(), true),
                AccountMeta::new(self.market, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    pub fn set_allow_outside_depositors(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        flag: bool,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetAllowOutsideDepositorsIx { vault_idx, flag }.data(),
            vec![
                AccountMeta::new_readonly(signer.pubkey(), true),
                AccountMeta::new(self.market, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    pub fn set_outside_deposits_approved(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
        flag: bool,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetOutsideDepositsApprovedIx { vault_idx, flag }.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// Leader seed / top-up. Creates + funds the leader's ATAs for the
    /// requested legs first, then sends `deposit_leader` as `authority`.
    pub fn deposit_leader(
        &mut self,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<(), String> {
        let auth = self.authority.insecure_clone();
        self.deposit_leader_as(
            &auth,
            vault_idx,
            base_in,
            quote_in,
            max_base_in,
            max_quote_in,
        )
    }

    /// Like [`Self::deposit_leader`] but signed by an arbitrary
    /// `signer` — for the `signer != vault.leader` rejection path.
    pub fn deposit_leader_as(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<(), String> {
        let leader = signer.pubkey();
        let (leader_base, leader_quote) = self.create_atas(&leader);
        let auth = self.authority.insecure_clone();
        // Fund up to the slippage caps, not just the sized leg: a
        // single-leg-sized top-up still pulls the proportional *other*
        // leg as part of the basket, and the basket is bounded above by
        // `max_*_in`. Minting the caps guarantees the leader can cover
        // whatever basket the handler derives.
        if max_base_in > 0 {
            mint_to(
                &mut self.svm,
                &auth,
                &self.base_mint,
                &leader_base,
                max_base_in,
            );
        }
        if max_quote_in > 0 {
            mint_to(
                &mut self.svm,
                &auth,
                &self.quote_mint,
                &leader_quote,
                max_quote_in,
            );
        }
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &DepositLeaderIx {
                vault_idx,
                base_in,
                quote_in,
                max_base_in,
                max_quote_in,
            }
            .data(),
            vec![
                AccountMeta::new(leader, true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(leader_base, false),
                AccountMeta::new(leader_quote, false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    pub fn withdraw_leader(
        &mut self,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<(), String> {
        let auth = self.authority.insecure_clone();
        self.withdraw_leader_as(&auth, vault_idx, shares_in, min_base_out, min_quote_out)
    }

    /// Like [`Self::withdraw_leader`] but signed by an arbitrary
    /// `signer` — for the `signer != vault.leader` rejection path.
    pub fn withdraw_leader_as(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<(), String> {
        let leader = signer.pubkey();
        let (leader_base, leader_quote) = self.create_atas(&leader);
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &WithdrawLeaderIx {
                vault_idx,
                shares_in,
                min_base_out,
                min_quote_out,
            }
            .data(),
            vec![
                AccountMeta::new(leader, true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(leader_base, false),
                AccountMeta::new(leader_quote, false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    /// Outside-depositor deposit. `depositor` signs and pays; its ATAs
    /// must already hold the legs (use [`Self::funded_depositor`]).
    pub fn deposit(
        &mut self,
        depositor: &Keypair,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<(), String> {
        let (vd, _) = vault_depositor_pda(&self.market, vault_idx, &depositor.pubkey());
        let b_ata = self.base_ata(&depositor.pubkey());
        let q_ata = self.quote_ata(&depositor.pubkey());
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &DepositIx {
                vault_idx,
                base_in,
                quote_in,
                max_base_in,
                max_quote_in,
            }
            .data(),
            vec![
                AccountMeta::new(depositor.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(vd, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(b_ata, false),
                AccountMeta::new(q_ata, false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, depositor, ix)
    }

    /// Outside-depositor withdraw.
    pub fn withdraw(
        &mut self,
        depositor: &Keypair,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<(), String> {
        let (vd, _) = vault_depositor_pda(&self.market, vault_idx, &depositor.pubkey());
        let b_ata = self.base_ata(&depositor.pubkey());
        let q_ata = self.quote_ata(&depositor.pubkey());
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &WithdrawIx {
                vault_idx,
                shares_in,
                min_base_out,
                min_quote_out,
            }
            .data(),
            vec![
                AccountMeta::new(depositor.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(vd, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(b_ata, false),
                AccountMeta::new(q_ata, false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, depositor, ix)
    }

    /// Build a `swap` instruction (not sent) — lets a test bundle it or
    /// assert on the raw error. `taker` must own the funded ATAs.
    pub fn swap_ix(
        &self,
        taker: &Pubkey,
        side: u8,
        amount_in: u64,
        limit_price_bits: u32,
        min_out: u64,
    ) -> Instruction {
        Instruction::new_with_bytes(
            PROGRAM_ID,
            &SwapIx {
                side,
                amount_in,
                limit_price_bits,
                min_out,
            }
            .data(),
            vec![
                AccountMeta::new(*taker, true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(self.base_ata(taker), false),
                AccountMeta::new(self.quote_ata(taker), false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(SYSVAR_CLOCK_ID, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        )
    }

    pub fn swap(
        &mut self,
        taker: &Keypair,
        side: u8,
        amount_in: u64,
        limit_price_bits: u32,
        min_out: u64,
    ) -> Result<(), String> {
        let ix = self.swap_ix(&taker.pubkey(), side, amount_in, limit_price_bits, min_out);
        send_ixn(&mut self.svm, taker, ix)
    }

    // ── lifecycle / teardown senders ─────────────────────────────────

    /// `close_vault` — leader moves their vault to the tombstone DLL.
    pub fn close_vault(&mut self, signer: &Keypair, vault_idx: u32) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CloseVaultIx { vault_idx }.data(),
            vec![
                AccountMeta::new_readonly(signer.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, signer, ix)
    }

    /// `freeze_vault` — admin freezes a vault in place.
    pub fn freeze_vault(&mut self, admin: &Keypair, vault_idx: u32) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &FreezeVaultIx { vault_idx }.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `force_withdraw_depositor` — admin drains `owner`'s full position
    /// and closes their PDA, refunding its rent to `owner`. The admin
    /// pays for the owner's payout ATAs if `init_if_needed` allocates
    /// them — here they already exist (the depositor was funded).
    pub fn force_withdraw_depositor(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
        owner: &Pubkey,
    ) -> Result<(), String> {
        let (vd, _) = vault_depositor_pda(&self.market, vault_idx, owner);
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &ForceWithdrawDepositorIx { vault_idx }.data(),
            vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new(*owner, false),
                AccountMeta::new(vd, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(self.base_ata(owner), false),
                AccountMeta::new(self.quote_ata(owner), false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `force_withdraw_leader` — admin drains the vault's leader stake to
    /// the leader's ATAs. On full drain the sector reclaims to the free
    /// DLL.
    pub fn force_withdraw_leader(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
        leader: &Pubkey,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &ForceWithdrawLeaderIx { vault_idx }.data(),
            vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(*leader, false),
                AccountMeta::new_readonly(self.base_mint, false),
                AccountMeta::new_readonly(self.quote_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(self.base_ata(leader), false),
                AccountMeta::new(self.quote_ata(leader), false),
                AccountMeta::new(self.base_treasury, false),
                AccountMeta::new(self.quote_treasury, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `close_market_treasury` — close one market treasury ATA, sending
    /// its rent to `rent_recipient`. `mint` selects the leg; `treasury`
    /// is that leg's treasury ATA.
    pub fn close_market_treasury(
        &mut self,
        admin: &Keypair,
        mint: &Pubkey,
        treasury: &Pubkey,
        rent_recipient: &Pubkey,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CloseMarketTreasuryIx {}.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new_readonly(self.market, false),
                AccountMeta::new_readonly(*mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(*treasury, false),
                AccountMeta::new(*rent_recipient, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `close_market` — close the market PDA + vault slab, refunding rent
    /// to `rent_recipient` and decrementing `registry.market_count`.
    pub fn close_market(&mut self, admin: &Keypair, rent_recipient: &Pubkey) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CloseMarketIx {}.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(self.base_treasury, false),
                AccountMeta::new_readonly(self.quote_treasury, false),
                AccountMeta::new(*rent_recipient, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `close_registry_fee_vault` — close the registry fee ATA, refunding
    /// rent to `rent_recipient`.
    pub fn close_registry_fee_vault(
        &mut self,
        admin: &Keypair,
        rent_recipient: &Pubkey,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CloseRegistryFeeVaultIx {}.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new_readonly(self.fee_mint, false),
                AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
                AccountMeta::new(self.registry_fee_treasury, false),
                AccountMeta::new(*rent_recipient, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    /// `close_registry` — close the registry PDA, refunding rent to
    /// `rent_recipient`.
    pub fn close_registry(
        &mut self,
        admin: &Keypair,
        rent_recipient: &Pubkey,
    ) -> Result<(), String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CloseRegistryIx {}.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new(self.registry, false),
                AccountMeta::new(*rent_recipient, false),
            ],
        );
        send_ixn(&mut self.svm, admin, ix)
    }

    // ── account-data readers ─────────────────────────────────────────

    pub fn token_balance(&self, ata: &Pubkey) -> u64 {
        let acct = self.svm.get_account(ata).expect("token account exists");
        u64::from_le_bytes(acct.data[64..72].try_into().unwrap())
    }

    /// `registry.market_count` — the live-market witness `close_registry`
    /// gates on.
    pub fn registry_market_count(&self) -> u32 {
        let acct = self.svm.get_account(&self.registry).expect("registry");
        bytemuck::pod_read_unaligned::<RegistryHeader>(
            &acct.data[8..8 + core::mem::size_of::<RegistryHeader>()],
        )
        .market_count
        .get()
    }

    pub fn market_header(&self) -> MarketHeader {
        let acct = self.svm.get_account(&self.market).expect("market");
        bytemuck::pod_read_unaligned::<MarketHeader>(
            &acct.data[8..8 + core::mem::size_of::<MarketHeader>()],
        )
    }

    pub fn vault(&self, sector_idx: u32) -> Vault {
        let acct = self.svm.get_account(&self.market).expect("market");
        let off = vault_byte_offset(sector_idx);
        bytemuck::pod_read_unaligned::<Vault>(&acct.data[off..off + core::mem::size_of::<Vault>()])
    }

    /// `Some(header)` if the outside-depositor PDA exists (i.e. was
    /// init'd and not yet closed), else `None`.
    pub fn vault_depositor(&self, vault_idx: u32, owner: &Pubkey) -> Option<VaultDepositorHeader> {
        let (vd, _) = vault_depositor_pda(&self.market, vault_idx, owner);
        let acct = self.svm.get_account(&vd)?;
        if acct.data.len() < 8 + core::mem::size_of::<VaultDepositorHeader>() {
            return None;
        }
        Some(bytemuck::pod_read_unaligned::<VaultDepositorHeader>(
            &acct.data[8..8 + core::mem::size_of::<VaultDepositorHeader>()],
        ))
    }

    // ── state pokes (no admin ix exists yet) ─────────────────────────

    /// Overwrite raw bytes inside vault `sector_idx` at
    /// `offset_of!(Vault, <field>)` and reinstall the account.
    fn poke_vault_bytes(&mut self, sector_idx: u32, field_offset: usize, bytes: &[u8]) {
        let mut acct = self.svm.get_account(&self.market).expect("market");
        let off = vault_byte_offset(sector_idx) + field_offset;
        acct.data[off..off + bytes.len()].copy_from_slice(bytes);
        self.svm.set_account(self.market, acct).expect("set market");
    }

    /// Set `Vault.min_leader_share` (ppm) directly (no
    /// `SetMinLeaderShare` ix yet) — lets a test arm the
    /// skin-in-the-game floor without threading an admin instruction.
    pub fn poke_min_leader_share(&mut self, sector_idx: u32, ppm: u32) {
        self.poke_vault_bytes(
            sector_idx,
            core::mem::offset_of!(Vault, min_leader_share),
            &ppm.to_le_bytes(),
        );
    }

    /// Zero a vault's `leader` (the free-list emptiness marker) so a
    /// live, in-range sector reads as empty — there is no `CloseVault`
    /// ix yet to vacate a sector the normal way, so this is how a test
    /// reaches the `VaultEmpty` rejection branch.
    pub fn poke_leader_empty(&mut self, sector_idx: u32) {
        self.poke_vault_bytes(sector_idx, core::mem::offset_of!(Vault, leader), &[0u8; 32]);
    }

    /// Set `MarketHeader.taker_fee` directly (no `SetMarketFeeConfig`
    /// ix yet).
    pub fn poke_taker_fee(&mut self, fee_ppm: u16) {
        let mut acct = self.svm.get_account(&self.market).expect("market");
        let off = 8 + core::mem::offset_of!(MarketHeader, taker_fee);
        acct.data[off..off + 2].copy_from_slice(&fee_ppm.to_le_bytes());
        self.svm.set_account(self.market, acct).expect("set market");
    }
}
