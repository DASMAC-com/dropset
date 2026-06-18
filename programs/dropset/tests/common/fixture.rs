// cspell:word idempotently
//! Shared market-bootstrap fixture and per-instruction ix-builders.
//!
//! Every integration test that needs a live market repeated the same
//! `init → create_market → create_vault → set_reference_price →
//! set_liquidity_profile → seed` plumbing (150-300 lines apiece). This
//! module collapses that to a handful of calls on a [`Fixture`] that
//! owns the `LiteSVM` and every derived handle, plus thin ix-builders
//! so a test reads as intent ("deposit, then withdraw half") rather
//! than `AccountMeta` lists.
//!
//! The `poke_*` helpers rewrite a single field directly on the account
//! and reinstall it, reaching states without threading a full
//! instruction. Some now have a real admin instruction
//! (`min_leader_share` → [`Fixture::set_min_leader_share`], `frozen` →
//! [`Fixture::freeze_vault`]); the pokes stay for the cases that still
//! need an out-of-band write — e.g. arming `min_leader_share` to an
//! arbitrary value without exercising the admin gate. `taker_fee` now
//! has a real lever ([`Fixture::set_taker_fee`]), so its poke is gone.

#![allow(dead_code)]

use super::{
    associated_token_address, create_associated_token_account, create_mock_usdc_mint,
    create_spl_mint, deploy_with_authority, mint_to, send_ixn, send_ixn_meta, ATA_PROGRAM_ID,
    CREATE_MARKET_FEE_ATOMS, PROGRAM_ID, SIGNER_FUNDING_LAMPORTS, SPL_TOKEN_PROGRAM_ID,
};
use anchor_lang_v2::{bytemuck, programs::System, Id, InstructionData};
use anchor_v2_testing::{Keypair, LiteSVM, Signer};
use dropset::{
    instruction::{
        CloseMarket as CloseMarketIx, CloseMarketTreasury as CloseMarketTreasuryIx,
        CloseRegistry as CloseRegistryIx, CloseRegistryFeeVault as CloseRegistryFeeVaultIx,
        CloseVault as CloseVaultIx, CreateMarket as CreateMarketIx, CreateVault as CreateVaultIx,
        Deposit as DepositIx, DepositLeader as DepositLeaderIx,
        ForceWithdrawDepositor as ForceWithdrawDepositorIx,
        ForceWithdrawLeader as ForceWithdrawLeaderIx, FreezeVault as FreezeVaultIx, Init as InitIx,
        SetAllowOutsideDepositors as SetAllowOutsideDepositorsIx,
        SetLiquidityProfile as SetLiquidityProfileIx, SetMarketFeeConfig as SetMarketFeeConfigIx,
        SetMinLeaderShare as SetMinLeaderShareIx,
        SetOutsideDepositsApproved as SetOutsideDepositsApprovedIx,
        SetReferencePrice as SetReferencePriceIx, SetRegistryDefaults as SetRegistryDefaultsIx,
        SetTakerFee as SetTakerFeeIx, Swap as SwapIx, Withdraw as WithdrawIx,
        WithdrawLeader as WithdrawLeaderIx,
    },
    Level, LiquidityProfile, MarketHeader, Price, RegistryHeader, Vault, VaultDepositorHeader,
    N_LEVELS,
};
use litesvm::types::TransactionMetadata;
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

/// Build a multi-level profile from explicit per-level
/// `(offset_ppm, size_bps, expiry_offset)` tuples — asks and bids fill from
/// level 0, unused slots stay zeroed. The ladder generalization of
/// [`simple_profile`], for matcher scenarios that need more than one live
/// level a side.
pub fn ladder_profile(asks: &[(u32, u16, u32)], bids: &[(u32, u16, u32)]) -> [u8; PROFILE_BYTES] {
    let mut profile: LiquidityProfile = bytemuck::Zeroable::zeroed();
    for (i, &(offset_ppm, size_bps, expiry_offset)) in asks.iter().enumerate() {
        profile.asks[i].price_offset = offset_ppm.into();
        profile.asks[i].size_bps = size_bps.into();
        profile.asks[i].expiry_offset = expiry_offset.into();
    }
    for (i, &(offset_ppm, size_bps, expiry_offset)) in bids.iter().enumerate() {
        profile.bids[i].price_offset = offset_ppm.into();
        profile.bids[i].size_bps = size_bps.into();
        profile.bids[i].expiry_offset = expiry_offset.into();
    }
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
    /// `init` the registry and `create_market` a fresh
    /// base/quote pair. No vault yet — call [`Self::create_vault`].
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
                fee_atoms: CREATE_MARKET_FEE_ATOMS,
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

        // create_market.
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
            &CreateMarketIx {}.data(),
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
        send_ixn(&mut svm, &authority, ix).expect("create_market");

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
        Self::seeded_with(base, quote, simple_profile(5_000, 10_000, u32::MAX))
    }

    /// Like [`Self::seeded`] but with a caller-supplied liquidity profile —
    /// for matcher scenarios that need a multi-level ladder (see
    /// [`ladder_profile`]). Same 1.0850 reference anchor and `quote_slot` 0;
    /// seeds `(base, quote)` via `deposit_leader`.
    pub fn seeded_with(base: u64, quote: u64, profile: [u8; PROFILE_BYTES]) -> Self {
        let mut f = Self::bootstrap();
        f.create_vault(0, f.authority.pubkey(), false, Pubkey::default())
            .expect("create_vault");
        let ref_price = Price::encode(10_850_000, 0).unwrap();
        f.set_reference_price(&f.authority.insecure_clone(), 0, ref_price.as_u32(), 0)
            .expect("set_reference_price");
        f.set_liquidity_profile(&f.authority.insecure_clone(), 0, profile)
            .expect("set_liquidity_profile");
        f.deposit_leader(0, base, quote, base, quote)
            .expect("seed deposit_leader");
        f
    }

    /// Bootstrap → open one outside-deposit-enabled vault (sector 0) led
    /// by a **distinct** keypair (not the admin) → seed it → satisfy the
    /// two-key gate → take one outside deposit from a fresh `alice`.
    /// Returns `(leader, alice)`.
    ///
    /// The shape the teardown depositor paths need: a live
    /// `VaultDepositor` PDA (so `force_withdraw_depositor`'s accounts
    /// resolve) plus a leader that never aliases the admin (Anchor v2
    /// rejects duplicate mutable accounts on `force_withdraw_leader`).
    /// Mirrors the build-up in `teardown.rs`'s headline test.
    pub fn with_outside_depositor(&mut self) -> (Keypair, Keypair) {
        let admin = self.authority.insecure_clone();
        let leader = self.funded_keypair(10 * SIGNER_FUNDING_LAMPORTS);
        self.create_vault(0, leader.pubkey(), true, leader.pubkey())
            .expect("admin opens leader's vault");
        let px = Price::encode(10_850_000, 0).unwrap();
        self.set_reference_price(&leader, 0, px.as_u32(), 0)
            .expect("leader sets reference price");
        self.set_liquidity_profile(&leader, 0, simple_profile(5_000, 10_000, u32::MAX))
            .expect("leader sets ladder");
        self.deposit_leader_as(&leader, 0, 1_000_000, 1_085_000, 1_000_000, 1_085_000)
            .expect("leader seeds the vault");
        self.set_outside_deposits_approved(&admin, 0, true)
            .expect("admin approves");
        let alice = self.funded_depositor(200_000, 200_000);
        self.deposit(&alice, 0, 50_000, 0, 200_000, 200_000)
            .expect("outside deposit");
        (leader, alice)
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

    /// `create_vault` via the admin path (payer = `authority`, fee
    /// waived). Returns `Err(debug-string)` on program rejection.
    pub fn create_vault(
        &mut self,
        perf_fee_rate: u32,
        quote_authority: Pubkey,
        allow_outside_depositors: bool,
        leader_override: Pubkey,
    ) -> Result<(), String> {
        self.create_vault_meta(
            perf_fee_rate,
            quote_authority,
            allow_outside_depositors,
            leader_override,
        )
        .map(|_| ())
    }

    /// Like [`Self::create_vault`] but yields the transaction metadata
    /// so a test can decode the emitted `CreateVaultEvent`.
    pub fn create_vault_meta(
        &mut self,
        perf_fee_rate: u32,
        quote_authority: Pubkey,
        allow_outside_depositors: bool,
        leader_override: Pubkey,
    ) -> Result<TransactionMetadata, String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CreateVaultIx {
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
        send_ixn_meta(&mut self.svm, &auth, ix)
    }

    /// `create_vault` via the admin path, but charging against an
    /// explicit `(fee_mint, fee_token_program)` rather than the bootstrap
    /// default — for asserting that after `set_market_fee_config` points a
    /// market at a fresh mint, the matching registry fee ATA already
    /// exists and `create_vault` loads it. The fee is waived (admin), so
    /// `payer_fee_source` is the never-read `dummy`.
    pub fn create_vault_with_fee(
        &mut self,
        perf_fee_rate: u32,
        quote_authority: Pubkey,
        allow_outside_depositors: bool,
        leader_override: Pubkey,
        fee_mint: &Pubkey,
        fee_token_program: &Pubkey,
    ) -> Result<(), String> {
        let registry_fee_treasury =
            associated_token_address(&self.registry, fee_mint, fee_token_program);
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CreateVaultIx {
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
                AccountMeta::new_readonly(*fee_mint, false),
                AccountMeta::new_readonly(*fee_token_program, false),
                AccountMeta::new(self.dummy.pubkey(), false),
                AccountMeta::new(registry_fee_treasury, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        let auth = self.authority.insecure_clone();
        send_ixn(&mut self.svm, &auth, ix)
    }

    /// `create_vault` via the **non-admin** fee path: `payer` signs
    /// and pays the per-market create-vault fee out of its fee-mint ATA
    /// (funded here with exactly the fee amount). Returns
    /// `Err(debug-string)` on program rejection.
    pub fn create_vault_as(
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
            CREATE_MARKET_FEE_ATOMS,
        );
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &CreateVaultIx {
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

    /// ATA holding the create-vault fee mint for `owner`.
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

    /// `set_min_leader_share` — admin retunes a vault's skin-in-the-game
    /// floor (ppm). Returns `Err(debug-string)` on program rejection.
    pub fn set_min_leader_share(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
        min_leader_share: u32,
    ) -> Result<(), String> {
        self.set_min_leader_share_meta(admin, vault_idx, min_leader_share)
            .map(|_| ())
    }

    /// Like [`Self::set_min_leader_share`] but yields the transaction
    /// metadata so a test can decode the emitted `SetMinLeaderShareEvent`.
    pub fn set_min_leader_share_meta(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
        min_leader_share: u32,
    ) -> Result<TransactionMetadata, String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetMinLeaderShareIx {
                vault_idx,
                min_leader_share,
            }
            .data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn_meta(&mut self.svm, admin, ix)
    }

    /// `set_market_fee_config` — admin retunes the market's per-`CreateVault`
    /// fee. `fee_mint` / `fee_token_program` are passed explicitly so a
    /// negative test can drive the `mint::token_program` mismatch.
    pub fn set_market_fee_config(
        &mut self,
        admin: &Keypair,
        fee_mint: &Pubkey,
        fee_token_program: &Pubkey,
        atoms: u64,
    ) -> Result<(), String> {
        self.set_market_fee_config_meta(admin, fee_mint, fee_token_program, atoms)
            .map(|_| ())
    }

    /// Like [`Self::set_market_fee_config`] but yields the transaction
    /// metadata so a test can decode the emitted `SetMarketFeeConfigEvent`.
    pub fn set_market_fee_config_meta(
        &mut self,
        admin: &Keypair,
        fee_mint: &Pubkey,
        fee_token_program: &Pubkey,
        atoms: u64,
    ) -> Result<TransactionMetadata, String> {
        // Registry fee ATA for the new mint — the instruction creates it
        // (`init_if_needed`) so the fee destination exists before the next
        // `create_vault` charges into it.
        let registry_fee_treasury =
            associated_token_address(&self.registry, fee_mint, fee_token_program);
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetMarketFeeConfigIx { atoms }.data(),
            vec![
                AccountMeta::new(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(*fee_mint, false),
                AccountMeta::new_readonly(*fee_token_program, false),
                AccountMeta::new(registry_fee_treasury, false),
                AccountMeta::new_readonly(ATA_PROGRAM_ID, false),
                AccountMeta::new_readonly(System::id(), false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn_meta(&mut self.svm, admin, ix)
    }

    /// `set_taker_fee` — admin retunes the market's taker fee (ppm,
    /// `Ppm16`). The real instruction that replaces the old
    /// `poke_taker_fee` out-of-band write.
    pub fn set_taker_fee(&mut self, admin: &Keypair, taker_fee: u16) -> Result<(), String> {
        self.set_taker_fee_meta(admin, taker_fee).map(|_| ())
    }

    /// Like [`Self::set_taker_fee`] but yields the transaction metadata so
    /// a test can decode the emitted `SetTakerFeeEvent`.
    pub fn set_taker_fee_meta(
        &mut self,
        admin: &Keypair,
        taker_fee: u16,
    ) -> Result<TransactionMetadata, String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetTakerFeeIx { taker_fee }.data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new_readonly(self.registry, false),
                AccountMeta::new(self.market, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn_meta(&mut self.svm, admin, ix)
    }

    /// `set_registry_defaults` — admin retunes the registry-wide scalar
    /// defaults future markets inherit. `None` leaves a field untouched.
    pub fn set_registry_defaults(
        &mut self,
        admin: &Keypair,
        taker_fee: Option<u16>,
        min_leader_share: Option<u32>,
    ) -> Result<(), String> {
        self.set_registry_defaults_meta(admin, taker_fee, min_leader_share)
            .map(|_| ())
    }

    /// Like [`Self::set_registry_defaults`] but yields the transaction
    /// metadata so a test can decode the emitted `SetRegistryDefaultsEvent`.
    pub fn set_registry_defaults_meta(
        &mut self,
        admin: &Keypair,
        taker_fee: Option<u16>,
        min_leader_share: Option<u32>,
    ) -> Result<TransactionMetadata, String> {
        let ix = Instruction::new_with_bytes(
            PROGRAM_ID,
            &SetRegistryDefaultsIx {
                taker_fee,
                min_leader_share,
            }
            .data(),
            vec![
                AccountMeta::new_readonly(admin.pubkey(), true),
                AccountMeta::new(self.registry, false),
                AccountMeta::new_readonly(event_authority(), false),
                AccountMeta::new_readonly(PROGRAM_ID, false),
            ],
        );
        send_ixn_meta(&mut self.svm, admin, ix)
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
        self.deposit_leader_as_meta(
            signer,
            vault_idx,
            base_in,
            quote_in,
            max_base_in,
            max_quote_in,
        )
        .map(|_| ())
    }

    /// Like [`Self::deposit_leader_as`] but yields the transaction
    /// metadata so a test can decode the emitted `DepositEvent` (and any
    /// `RealizeEvent`).
    pub fn deposit_leader_as_meta(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, signer, ix)
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
        self.withdraw_leader_as_meta(signer, vault_idx, shares_in, min_base_out, min_quote_out)
            .map(|_| ())
    }

    /// Like [`Self::withdraw_leader_as`] but yields the transaction
    /// metadata so a test can decode the emitted `WithdrawEvent`.
    pub fn withdraw_leader_as_meta(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, signer, ix)
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
        self.deposit_meta(
            depositor,
            vault_idx,
            base_in,
            quote_in,
            max_base_in,
            max_quote_in,
        )
        .map(|_| ())
    }

    /// Like [`Self::deposit`] but yields the transaction metadata so a
    /// test can decode the emitted `DepositEvent` (and any `RealizeEvent`).
    pub fn deposit_meta(
        &mut self,
        depositor: &Keypair,
        vault_idx: u32,
        base_in: u64,
        quote_in: u64,
        max_base_in: u64,
        max_quote_in: u64,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, depositor, ix)
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
        self.withdraw_meta(depositor, vault_idx, shares_in, min_base_out, min_quote_out)
            .map(|_| ())
    }

    /// Like [`Self::withdraw`] but yields the transaction metadata so a
    /// test can decode the emitted `WithdrawEvent` (and any `RealizeEvent`).
    pub fn withdraw_meta(
        &mut self,
        depositor: &Keypair,
        vault_idx: u32,
        shares_in: u64,
        min_base_out: u64,
        min_quote_out: u64,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, depositor, ix)
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
        self.swap_meta(taker, side, amount_in, limit_price_bits, min_out)
            .map(|_| ())
    }

    /// Like [`Self::swap`] but yields the transaction metadata so a test
    /// can decode the per-leg `FillEvent`s.
    pub fn swap_meta(
        &mut self,
        taker: &Keypair,
        side: u8,
        amount_in: u64,
        limit_price_bits: u32,
        min_out: u64,
    ) -> Result<TransactionMetadata, String> {
        let ix = self.swap_ix(&taker.pubkey(), side, amount_in, limit_price_bits, min_out);
        send_ixn_meta(&mut self.svm, taker, ix)
    }

    // ── lifecycle / teardown senders ─────────────────────────────────

    /// `close_vault` — leader moves their vault to the tombstone DLL.
    pub fn close_vault(&mut self, signer: &Keypair, vault_idx: u32) -> Result<(), String> {
        self.close_vault_meta(signer, vault_idx).map(|_| ())
    }

    /// Like [`Self::close_vault`] but yields the transaction metadata so
    /// a test can decode the emitted `CloseVaultEvent`.
    pub fn close_vault_meta(
        &mut self,
        signer: &Keypair,
        vault_idx: u32,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, signer, ix)
    }

    /// `freeze_vault` — admin freezes a vault in place.
    pub fn freeze_vault(&mut self, admin: &Keypair, vault_idx: u32) -> Result<(), String> {
        self.freeze_vault_meta(admin, vault_idx).map(|_| ())
    }

    /// Like [`Self::freeze_vault`] but yields the transaction metadata so
    /// a test can decode the emitted `FreezeVaultEvent`.
    pub fn freeze_vault_meta(
        &mut self,
        admin: &Keypair,
        vault_idx: u32,
    ) -> Result<TransactionMetadata, String> {
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
        send_ixn_meta(&mut self.svm, admin, ix)
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

    /// Full [`RegistryHeader`] — for asserting the stamped defaults
    /// (`default_taker_fee`, `default_min_leader_share`) after a
    /// `set_registry_defaults` call.
    pub fn registry_header(&self) -> RegistryHeader {
        let acct = self.svm.get_account(&self.registry).expect("registry");
        bytemuck::pod_read_unaligned::<RegistryHeader>(
            &acct.data[8..8 + core::mem::size_of::<RegistryHeader>()],
        )
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

    /// Overwrite `Vault.profile.{asks,bids}[level].size_bps` for vault
    /// `sector_idx`, bypassing the `set_liquidity_profile` per-side Σ ≤ BPS
    /// bound (`is_ask` selects the side). No
    /// instruction writes a `size_bps > BPS`, so this is the only way to
    /// reach the matcher's out-of-range flush-size branch — the on-chain
    /// hard-reject (`LiquidityProfileSizeOverflow`) and the simulator's
    /// empty-quote mirror.
    pub fn poke_level_size_bps(
        &mut self,
        sector_idx: u32,
        is_ask: bool,
        level: usize,
        size_bps: u16,
    ) {
        let side_off = if is_ask {
            core::mem::offset_of!(LiquidityProfile, asks)
        } else {
            core::mem::offset_of!(LiquidityProfile, bids)
        };
        let field_offset = core::mem::offset_of!(Vault, profile)
            + side_off
            + level * core::mem::size_of::<Level>()
            + core::mem::offset_of!(Level, size_bps);
        self.poke_vault_bytes(sector_idx, field_offset, &size_bps.to_le_bytes());
    }

    /// Overwrite `Vault.next` (the active-DLL forward pointer) for vault
    /// `sector_idx`. The list ops keep `next` acyclic and in-bounds, so a
    /// cyclic (e.g. self-referential) or out-of-range pointer is only
    /// reachable by poking corrupt bytes — the way to drive the matcher's
    /// `CorruptVaultList` walk guard and the simulator's empty-quote
    /// mirror.
    pub fn poke_vault_next(&mut self, sector_idx: u32, next: u32) {
        self.poke_vault_bytes(
            sector_idx,
            core::mem::offset_of!(Vault, next),
            &next.to_le_bytes(),
        );
    }

    /// Set `Vault.min_leader_share` (ppm) directly — lets a test arm the
    /// skin-in-the-game floor without exercising the admin gate or the
    /// `<= PPM` cap the real [`Fixture::set_min_leader_share`] enforces.
    pub fn poke_min_leader_share(&mut self, sector_idx: u32, ppm: u32) {
        self.poke_vault_bytes(
            sector_idx,
            core::mem::offset_of!(Vault, min_leader_share),
            &ppm.to_le_bytes(),
        );
    }

    /// Zero a vault's `leader` (the free-list emptiness marker) so a
    /// live, in-range sector reads as empty — how a test reaches the
    /// `VaultEmpty` rejection branch. The honest way to a
    /// `leader == default` sector is `CloseVault` → full drain →
    /// `reclaim_sector` (which zeroes `leader` and returns the sector to
    /// the free DLL); no single instruction leaves a live-indexed sector
    /// reading empty. That close + drain + reclaim plumbing is heavier
    /// than a rejection test needs, so this poke is a deliberate shortcut.
    pub fn poke_leader_empty(&mut self, sector_idx: u32) {
        self.poke_vault_bytes(sector_idx, core::mem::offset_of!(Vault, leader), &[0u8; 32]);
    }

    /// Set `Vault.hwm` (value-per-share high-water mark, Q32.32) directly.
    /// Dropping it below the live value-per-share is how a test arms a
    /// `Realize` perf-fee accrual without plumbing a profitable swap —
    /// HWM lagging behind NAV is exactly the condition `realize_in_place`
    /// accrues on.
    pub fn poke_hwm(&mut self, sector_idx: u32, hwm: u64) {
        self.poke_vault_bytes(
            sector_idx,
            core::mem::offset_of!(Vault, hwm),
            &hwm.to_le_bytes(),
        );
    }

    /// Set `MarketHeader.nonce` directly. The nonce only advances one
    /// per quote/fill through normal instructions, so it can't reach
    /// `u64::MAX` in a test the honest way — poke it to drive the
    /// per-leg `checked_add(1)` overflow branch in `swap`.
    pub fn poke_nonce(&mut self, nonce: u64) {
        let mut acct = self.svm.get_account(&self.market).expect("market");
        let off = 8 + core::mem::offset_of!(MarketHeader, nonce);
        acct.data[off..off + 8].copy_from_slice(&nonce.to_le_bytes());
        self.svm.set_account(self.market, acct).expect("set market");
    }
}
