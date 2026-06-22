//! On-chain plumbing: PDA / ATA derivations, the bootstrap + teardown
//! instruction builders, mock-mint creation, and a sign-and-send helper.
//!
//! The instruction builders are thin wrappers over
//! [`dropset_sdk::instructions`] — the Codama-generated structs whose
//! field order *is* the canonical `AccountMeta` ordering (regenerated from
//! the same IDL as the on-chain program). So this module's only real job is
//! to derive the right PDA / ATA for each field; the ordering is inherited,
//! not re-asserted. The unit tests still pin the resulting metas against the
//! orderings in `programs/dropset/tests/common/fixture.rs` so a transposed
//! field (base where quote belongs) is caught at `cargo test`, not on chain.

use anyhow::{Context, Result};
use dropset_sdk::instructions::{
    CloseMarket, CloseMarketTreasury, CloseRegistry, CloseRegistryFeeVault, CreateMarket,
    CreateVault, CreateVaultInstructionArgs, ForceWithdrawDepositor,
    ForceWithdrawDepositorInstructionArgs, ForceWithdrawLeader, ForceWithdrawLeaderInstructionArgs,
    Init, InitInstructionArgs,
};
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_loader_v3_interface::get_program_data_address;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;

/// SPL Token program.
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program.
pub const ATA_PROGRAM_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// System program.
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

/// $1,000 in 6-decimal atoms — the per-`CreateVault` fee stamped at `init`.
/// Mirrors `CREATE_MARKET_FEE_ATOMS` in the test fixture. Waived on the
/// admin path the TUI uses, so it never actually charges the wallet.
pub const CREATE_MARKET_FEE_ATOMS: u64 = 1_000 * 1_000_000;

/// SPL Token Mint account size (bytes).
const MINT_LEN: usize = 82;

/// Default perf-fee rate (ppm) for the bootstrap vault — 0, a plain vault.
const DEFAULT_PERF_FEE_RATE: u32 = 0;

// ── RPC ──────────────────────────────────────────────────────────────

/// An `RpcClient` at the `confirmed` commitment, pointed at `url`.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_commitment(url.to_string(), CommitmentConfig::confirmed())
}

// ── PDA / ATA derivations ────────────────────────────────────────────

/// The singleton registry PDA — seeds `[b"registry"]`.
pub fn registry_pda() -> Pubkey {
    Pubkey::find_program_address(&[b"registry"], &DROPSET_ID).0
}

/// The market PDA for a `(base, quote)` mint pair — seeds `[base, quote]`.
pub fn market_pda(base: &Pubkey, quote: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[base.as_ref(), quote.as_ref()], &DROPSET_ID).0
}

/// The self-CPI event-authority PDA — seeds `[b"__event_authority"]`.
pub fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID).0
}

/// The program's upgradeable-loader `ProgramData` PDA — read by `init` to
/// authenticate the upgrade authority.
pub fn program_data() -> Pubkey {
    get_program_data_address(&DROPSET_ID)
}

/// Canonical associated-token-account address for `(wallet, mint,
/// token_program)` — seeds `[wallet, token_program, mint]` under the ATA
/// program.
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[wallet.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ATA_PROGRAM_ID,
    )
    .0
}

// ── Bootstrap instruction builders ───────────────────────────────────

/// `init` — create the registry, charging the per-`CreateVault` fee in
/// `fee_mint`. `payer` is the genesis admin (must equal the program's
/// upgrade authority).
pub fn build_init_ix(payer: &Pubkey, fee_mint: &Pubkey) -> Instruction {
    let registry = registry_pda();
    Init {
        payer: *payer,
        registry,
        program_data: program_data(),
        fee_mint: *fee_mint,
        fee_vault: associated_token_address(&registry, fee_mint, &SPL_TOKEN_PROGRAM_ID),
        token_program: SPL_TOKEN_PROGRAM_ID,
        associated_token_program: ATA_PROGRAM_ID,
        system_program: SYSTEM_PROGRAM_ID,
    }
    .instruction(InitInstructionArgs {
        genesis_admin: *payer,
        fee_atoms: CREATE_MARKET_FEE_ATOMS,
    })
}

/// `create_market` for a fresh `(base, quote)` pair. `fee_mint` /
/// `fee_token_program` come from the registry's stamped default fee config;
/// `payer` (an admin) has its fee waived, so `payer_fee_source` is unread.
pub fn build_create_market_ix(
    payer: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    fee_mint: &Pubkey,
    fee_token_program: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    let market = market_pda(base_mint, quote_mint);
    CreateMarket {
        payer: *payer,
        registry,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        base_token_program: SPL_TOKEN_PROGRAM_ID,
        quote_token_program: SPL_TOKEN_PROGRAM_ID,
        market,
        base_treasury: associated_token_address(&market, base_mint, &SPL_TOKEN_PROGRAM_ID),
        quote_treasury: associated_token_address(&market, quote_mint, &SPL_TOKEN_PROGRAM_ID),
        fee_mint: *fee_mint,
        fee_token_program: *fee_token_program,
        // Admin path skips the fee transfer, so this account is never read;
        // the wallet itself is a valid writable stand-in.
        payer_fee_source: *payer,
        registry_fee_treasury: associated_token_address(
            &registry,
            fee_mint,
            fee_token_program,
        ),
        system_program: SYSTEM_PROGRAM_ID,
        associated_token_program: ATA_PROGRAM_ID,
    }
    .instruction()
}

/// `create_vault` on `market` via the admin path (fee waived, `payer`
/// becomes the leader). `fee_mint` / `fee_token_program` are the market's
/// stamped fee config.
pub fn build_create_vault_ix(
    payer: &Pubkey,
    market: &Pubkey,
    fee_mint: &Pubkey,
    fee_token_program: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    CreateVault {
        payer: *payer,
        registry,
        market: *market,
        fee_mint: *fee_mint,
        fee_token_program: *fee_token_program,
        payer_fee_source: *payer,
        registry_fee_treasury: associated_token_address(
            &registry,
            fee_mint,
            fee_token_program,
        ),
        system_program: SYSTEM_PROGRAM_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(CreateVaultInstructionArgs {
        perf_fee_rate: DEFAULT_PERF_FEE_RATE,
        // Leader-only vault: the leader is also the quote authority, no
        // outside depositors, no distinct leader override.
        quote_authority: *payer,
        allow_outside_depositors: false,
        leader_override: Pubkey::default(),
    })
}

// ── Teardown instruction builders ────────────────────────────────────

/// `force_withdraw_depositor` — admin drains `owner`'s position on
/// `vault_idx` and closes their PDA. Only used when a market has outside
/// depositors; the TUI's own bootstrap never creates one.
pub fn build_force_withdraw_depositor_ix(
    admin: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    vault_idx: u32,
    owner: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    let (vault_depositor, _) = Pubkey::find_program_address(
        &[
            b"vault_depositor",
            market.as_ref(),
            &vault_idx.to_le_bytes(),
            owner.as_ref(),
        ],
        &DROPSET_ID,
    );
    ForceWithdrawDepositor {
        admin: *admin,
        registry,
        market: *market,
        owner: *owner,
        vault_depositor,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        base_token_program: SPL_TOKEN_PROGRAM_ID,
        quote_token_program: SPL_TOKEN_PROGRAM_ID,
        owner_base_ata: associated_token_address(owner, base_mint, &SPL_TOKEN_PROGRAM_ID),
        owner_quote_ata: associated_token_address(owner, quote_mint, &SPL_TOKEN_PROGRAM_ID),
        market_base_treasury: *base_treasury,
        market_quote_treasury: *quote_treasury,
        associated_token_program: ATA_PROGRAM_ID,
        system_program: SYSTEM_PROGRAM_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(ForceWithdrawDepositorInstructionArgs { vault_idx })
}

/// `force_withdraw_leader` — admin drains `leader`'s stake on `vault_idx`,
/// reclaiming the sector to the free list on a full drain.
#[allow(clippy::too_many_arguments)]
pub fn build_force_withdraw_leader_ix(
    admin: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    vault_idx: u32,
    leader: &Pubkey,
) -> Instruction {
    ForceWithdrawLeader {
        admin: *admin,
        registry: registry_pda(),
        market: *market,
        leader: *leader,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        base_token_program: SPL_TOKEN_PROGRAM_ID,
        quote_token_program: SPL_TOKEN_PROGRAM_ID,
        leader_base_ata: associated_token_address(leader, base_mint, &SPL_TOKEN_PROGRAM_ID),
        leader_quote_ata: associated_token_address(leader, quote_mint, &SPL_TOKEN_PROGRAM_ID),
        market_base_treasury: *base_treasury,
        market_quote_treasury: *quote_treasury,
        associated_token_program: ATA_PROGRAM_ID,
        system_program: SYSTEM_PROGRAM_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(ForceWithdrawLeaderInstructionArgs { vault_idx })
}

/// `close_market_treasury` — close one leg's treasury ATA, refunding its
/// rent to `rent_recipient`.
pub fn build_close_market_treasury_ix(
    admin: &Pubkey,
    market: &Pubkey,
    mint: &Pubkey,
    treasury: &Pubkey,
    rent_recipient: &Pubkey,
) -> Instruction {
    CloseMarketTreasury {
        admin: *admin,
        registry: registry_pda(),
        market: *market,
        mint: *mint,
        token_program: SPL_TOKEN_PROGRAM_ID,
        treasury: *treasury,
        rent_recipient: *rent_recipient,
    }
    .instruction()
}

/// `close_market` — close the market PDA + vault slab, refunding rent to
/// `rent_recipient` and decrementing `registry.market_count`.
pub fn build_close_market_ix(
    admin: &Pubkey,
    market: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    rent_recipient: &Pubkey,
) -> Instruction {
    CloseMarket {
        admin: *admin,
        registry: registry_pda(),
        market: *market,
        base_treasury: *base_treasury,
        quote_treasury: *quote_treasury,
        rent_recipient: *rent_recipient,
    }
    .instruction()
}

/// `close_registry_fee_vault` — close the registry's fee ATA for
/// `(fee_mint, token_program)`, refunding rent to `rent_recipient`.
pub fn build_close_registry_fee_vault_ix(
    admin: &Pubkey,
    fee_mint: &Pubkey,
    fee_token_program: &Pubkey,
    rent_recipient: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    CloseRegistryFeeVault {
        admin: *admin,
        registry,
        fee_mint: *fee_mint,
        token_program: *fee_token_program,
        fee_vault: associated_token_address(&registry, fee_mint, fee_token_program),
        rent_recipient: *rent_recipient,
    }
    .instruction()
}

/// `close_registry` — close the registry PDA, refunding rent to
/// `rent_recipient`. Rejected unless `market_count == 0`.
pub fn build_close_registry_ix(admin: &Pubkey, rent_recipient: &Pubkey) -> Instruction {
    CloseRegistry {
        admin: *admin,
        registry: registry_pda(),
        rent_recipient: *rent_recipient,
    }
    .instruction()
}

// ── Mock mint creation + send ────────────────────────────────────────

/// Create a 6-decimal SPL Token mint owned by `authority` and return its
/// pubkey. Ports `create_spl_mint` from the test fixture: a
/// `SystemProgram::CreateAccount` + `InitializeMint2` pair, signed by the
/// new mint keypair alongside the funding `authority`.
pub fn create_spl_mint(client: &RpcClient, authority: &Keypair) -> Result<Pubkey> {
    let mint = Keypair::new();
    let lamports = client
        .get_minimum_balance_for_rent_exemption(MINT_LEN)
        .context("rent for mint account")?;

    // SystemProgram::CreateAccount (index 0): lamports, space, owner.
    let mut create_data = Vec::with_capacity(4 + 8 + 8 + 32);
    create_data.extend_from_slice(&0u32.to_le_bytes());
    create_data.extend_from_slice(&lamports.to_le_bytes());
    create_data.extend_from_slice(&(MINT_LEN as u64).to_le_bytes());
    create_data.extend_from_slice(&SPL_TOKEN_PROGRAM_ID.to_bytes());
    let create = Instruction::new_with_bytes(
        SYSTEM_PROGRAM_ID,
        &create_data,
        vec![
            AccountMeta::new(authority.pubkey(), true),
            AccountMeta::new(mint.pubkey(), true),
        ],
    );

    // InitializeMint2 (index 20): decimals, mint authority, no freeze.
    let mut mint_data = vec![20u8, 6];
    mint_data.extend_from_slice(&authority.pubkey().to_bytes());
    mint_data.push(0);
    let init_mint = Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &mint_data,
        vec![AccountMeta::new(mint.pubkey(), false)],
    );

    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(
        &[create, init_mint],
        Some(&authority.pubkey()),
        &[authority, &mint],
        blockhash,
    );
    client
        .send_and_confirm_transaction(&tx)
        .context("create mint")?;
    Ok(mint.pubkey())
}

/// Sign `ixs` with `signers` (fee payer = `payer`) and send, confirming at
/// the client's commitment. Returns the transaction signature as a string.
pub fn send(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Result<String> {
    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    let sig = client
        .send_and_confirm_transaction(&tx)
        .context("send transaction")?;
    Ok(sig.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(is_signer, is_writable)` tuples for an instruction's metas, paired
    /// with the pubkey — the shape the fixture's `AccountMeta` lists encode.
    fn metas(ix: &Instruction) -> Vec<(Pubkey, bool, bool)> {
        ix.accounts
            .iter()
            .map(|m| (m.pubkey, m.is_signer, m.is_writable))
            .collect()
    }

    #[test]
    fn registry_pda_is_canonical() {
        assert_eq!(
            registry_pda(),
            Pubkey::find_program_address(&[b"registry"], &DROPSET_ID).0
        );
    }

    /// Pins the `init` account ordering against `fixture::init_ixn`:
    /// payer(signer,w) · registry(w) · program_data · fee_mint · fee_vault(w)
    /// · token_program · ata_program · system.
    #[test]
    fn init_ordering_matches_fixture() {
        let payer = Pubkey::new_unique();
        let fee_mint = Pubkey::new_unique();
        let registry = registry_pda();
        let fee_vault = associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
        let ix = build_init_ix(&payer, &fee_mint);
        assert_eq!(ix.program_id, DROPSET_ID);
        assert_eq!(
            metas(&ix),
            vec![
                (payer, true, true),
                (registry, false, true),
                (program_data(), false, false),
                (fee_mint, false, false),
                (fee_vault, false, true),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (ATA_PROGRAM_ID, false, false),
                (SYSTEM_PROGRAM_ID, false, false),
            ]
        );
    }

    /// Pins the `create_market` ordering against `fixture::bootstrap`'s
    /// create-market ix — in particular that base/quote mints and
    /// treasuries are not transposed.
    #[test]
    fn create_market_ordering_matches_fixture() {
        let payer = Pubkey::new_unique();
        let base = Pubkey::new_unique();
        let quote = Pubkey::new_unique();
        let fee_mint = Pubkey::new_unique();
        let registry = registry_pda();
        let market = market_pda(&base, &quote);
        let ix = build_create_market_ix(&payer, &base, &quote, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
        assert_eq!(
            metas(&ix),
            vec![
                (payer, true, true),
                (registry, false, true),
                (base, false, false),
                (quote, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (market, false, true),
                (
                    associated_token_address(&market, &base, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (
                    associated_token_address(&market, &quote, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (fee_mint, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (payer, false, true),
                (
                    associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (SYSTEM_PROGRAM_ID, false, false),
                (ATA_PROGRAM_ID, false, false),
            ]
        );
    }

    /// Pins the `create_vault` ordering (admin path) against
    /// `fixture::create_vault_meta`, including the trailing
    /// event_authority · program self-CPI pair.
    #[test]
    fn create_vault_ordering_matches_fixture() {
        let payer = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let fee_mint = Pubkey::new_unique();
        let registry = registry_pda();
        let ix = build_create_vault_ix(&payer, &market, &fee_mint, &SPL_TOKEN_PROGRAM_ID);
        assert_eq!(
            metas(&ix),
            vec![
                (payer, true, true),
                (registry, false, false),
                (market, false, true),
                (fee_mint, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (payer, false, true),
                (
                    associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (SYSTEM_PROGRAM_ID, false, false),
                (event_authority(), false, false),
                (DROPSET_ID, false, false),
            ]
        );
    }
}
