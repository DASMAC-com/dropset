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

use crate::job::Logger;
use anyhow::{Context, Result};
use dropset_sdk::instructions::{
    CloseMarket, CloseMarketTreasury, CloseRegistry, CloseRegistryFeeVault, CreateMarket,
    CreateVault, CreateVaultInstructionArgs, DepositLeader, DepositLeaderInstructionArgs,
    ForceWithdrawDepositor, ForceWithdrawDepositorInstructionArgs, ForceWithdrawLeader,
    ForceWithdrawLeaderInstructionArgs, Init, InitInstructionArgs, Swap, SwapInstructionArgs,
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
/// Clock sysvar — the `swap` instruction reads it for level-expiry checks.
pub const CLOCK_SYSVAR_ID: Pubkey = pubkey!("SysvarC1ock11111111111111111111111111111111");

/// $1,000 in 6-decimal atoms — the per-`CreateVault` fee stamped at `init`.
/// Mirrors `CREATE_MARKET_FEE_ATOMS` in the test fixture. Waived on the
/// admin path the TUI uses, so it never actually charges the wallet.
pub const CREATE_MARKET_FEE_ATOMS: u64 = 1_000 * 1_000_000;

/// SPL Token Mint account size (bytes).
const MINT_LEN: usize = 82;

/// Lamports the TUI pre-funds onto each program-created PDA (registry,
/// market) in the same transaction as its creation. anchor-v2 `init`
/// under-funds a `Slab` account against a real validator's post-execution
/// rent-exemption check — litesvm doesn't enforce that check, so the
/// program's tests never exercised it. Topping the account up in a trailing
/// `transfer` (the rent check runs once at end-of-transaction) carries it
/// over the rent floor. 0.02 SOL dwarfs the rent-exempt minimum of these
/// small accounts, so the result is exempt regardless of how little `init`
/// funded; the excess is reclaimed to the wallet at teardown. Interim
/// workaround pending a program-side fix to fund init rent correctly.
pub const RENT_TOPUP_LAMPORTS: u64 = 20_000_000;

/// Default perf-fee rate (ppm) for the bootstrap vault — 0, a plain vault.
const DEFAULT_PERF_FEE_RATE: u32 = 0;

// ── RPC ──────────────────────────────────────────────────────────────

/// An `RpcClient` at the `confirmed` commitment, pointed at `url`. The
/// short request timeout keeps a poll issued while the validator is still
/// booting from stalling the synchronous event loop.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        url.to_string(),
        std::time::Duration::from_secs(2),
        CommitmentConfig::confirmed(),
    )
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
/// `payer` (an admin) has its fee waived, so `fee_source` is never read —
/// but it must be a **distinct** account from `payer`, since anchor-v2
/// rejects the same key in two mutable slots
/// (`ConstraintDuplicateMutableAccount`).
pub fn build_create_market_ix(
    payer: &Pubkey,
    fee_source: &Pubkey,
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
        payer_fee_source: *fee_source,
        registry_fee_treasury: associated_token_address(&registry, fee_mint, fee_token_program),
        system_program: SYSTEM_PROGRAM_ID,
        associated_token_program: ATA_PROGRAM_ID,
    }
    .instruction()
}

/// `create_vault` on `market` via the admin path (fee waived). The vault is
/// opened for a **distinct** `leader` (via `leader_override`), not the admin
/// `payer` — admin teardown's `force_withdraw_leader` lists the leader
/// alongside the admin signer, and anchor-v2 rejects the same key in two
/// slots when one is mutable. `fee_source` likewise must differ from
/// `payer`. `fee_mint` / `fee_token_program` are the market's stamped fee
/// config.
pub fn build_create_vault_ix(
    payer: &Pubkey,
    fee_source: &Pubkey,
    market: &Pubkey,
    fee_mint: &Pubkey,
    fee_token_program: &Pubkey,
    leader: &Pubkey,
) -> Instruction {
    let registry = registry_pda();
    CreateVault {
        payer: *payer,
        registry,
        market: *market,
        fee_mint: *fee_mint,
        fee_token_program: *fee_token_program,
        payer_fee_source: *fee_source,
        registry_fee_treasury: associated_token_address(&registry, fee_mint, fee_token_program),
        system_program: SYSTEM_PROGRAM_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(CreateVaultInstructionArgs {
        perf_fee_rate: DEFAULT_PERF_FEE_RATE,
        quote_authority: *leader,
        allow_outside_depositors: false,
        leader_override: *leader,
    })
}

/// `deposit_leader` — the vault's `leader` (signer) seeds `(base_in,
/// quote_in)` atoms from its own ATAs into the market treasuries. The
/// basket is bounded above by `(base_in, quote_in)`; the leader's ATAs must
/// already hold the legs (mint to them first). Used by the bootstrap to
/// open the vault with live inventory.
#[allow(clippy::too_many_arguments)]
pub fn build_deposit_leader_ix(
    leader: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    vault_idx: u32,
    base_in: u64,
    quote_in: u64,
) -> Instruction {
    DepositLeader {
        signer: *leader,
        market: *market,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        base_token_program: SPL_TOKEN_PROGRAM_ID,
        quote_token_program: SPL_TOKEN_PROGRAM_ID,
        signer_base_ata: associated_token_address(leader, base_mint, &SPL_TOKEN_PROGRAM_ID),
        signer_quote_ata: associated_token_address(leader, quote_mint, &SPL_TOKEN_PROGRAM_ID),
        market_base_treasury: *base_treasury,
        market_quote_treasury: *quote_treasury,
        system_program: SYSTEM_PROGRAM_ID,
        associated_token_program: ATA_PROGRAM_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(DepositLeaderInstructionArgs {
        vault_idx,
        base_in,
        quote_in,
        max_base_in: base_in,
        max_quote_in: quote_in,
    })
}

/// `swap` — a taker take against the market's live book. `side` is `0` for a
/// Buy (pays quote from `taker`'s quote ATA, receives base) or `1` for a Sell
/// (pays base, receives quote); `limit_price_bits` is the worst acceptable
/// fill (`Price::INFINITY` bits for a Buy / `Price::ZERO` for a Sell disables
/// the bound) and `min_out` the slippage floor. Used by the TUI's swap probe
/// to exercise — and measure the CU of — a real take against the seeded vault.
#[allow(clippy::too_many_arguments)]
pub fn build_swap_ix(
    taker: &Pubkey,
    market: &Pubkey,
    base_mint: &Pubkey,
    quote_mint: &Pubkey,
    base_treasury: &Pubkey,
    quote_treasury: &Pubkey,
    side: u8,
    amount_in: u64,
    limit_price_bits: u32,
    min_out: u64,
) -> Instruction {
    Swap {
        taker: *taker,
        market: *market,
        base_mint: *base_mint,
        quote_mint: *quote_mint,
        base_token_program: SPL_TOKEN_PROGRAM_ID,
        quote_token_program: SPL_TOKEN_PROGRAM_ID,
        taker_base_ata: associated_token_address(taker, base_mint, &SPL_TOKEN_PROGRAM_ID),
        taker_quote_ata: associated_token_address(taker, quote_mint, &SPL_TOKEN_PROGRAM_ID),
        market_base_treasury: *base_treasury,
        market_quote_treasury: *quote_treasury,
        clock: CLOCK_SYSVAR_ID,
        event_authority: event_authority(),
        program: DROPSET_ID,
    }
    .instruction(SwapInstructionArgs {
        side,
        amount_in,
        limit_price_bits,
        min_out,
    })
}

// ── Teardown instruction builders ────────────────────────────────────

/// `force_withdraw_depositor` — admin drains `owner`'s position on
/// `vault_idx` and closes their PDA. Only used when a market has outside
/// depositors; the TUI's own bootstrap never creates one.
#[allow(clippy::too_many_arguments)]
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

/// Create a 6-decimal SPL Token mint at a fresh random address owned by
/// `authority`. The registry fee mint uses this — its address is incidental
/// (read back from the registry), unlike the traded pair's fixed mints.
pub fn create_spl_mint(client: &RpcClient, authority: &Keypair) -> Result<Pubkey> {
    create_mint(client, authority, &Keypair::new(), 6)
}

/// Create an SPL Token mint at `mint`'s address with `decimals`, mint
/// authority `authority`. Ports the test fixture's `create_spl_mint`: a
/// `SystemProgram::CreateAccount` + `InitializeMint2` pair, signed by the
/// `mint` keypair alongside the funding `authority`. The explicit keypair +
/// decimals let the bootstrap mint the fixed, checked-in pair mints (a
/// `PairConfig`'s `MintSpec`) at their deterministic addresses.
pub fn create_mint(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Keypair,
    decimals: u8,
) -> Result<Pubkey> {
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
    let mut mint_data = vec![20u8, decimals];
    mint_data.extend_from_slice(&authority.pubkey().to_bytes());
    mint_data.push(0);
    let init_mint = Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &mint_data,
        vec![AccountMeta::new(mint.pubkey(), false)],
    );

    send(client, authority, &[authority, mint], &[create, init_mint]).context("create mint")?;
    Ok(mint.pubkey())
}

/// Mint `amount` atoms of `mint` to `ata` under the SPL Token program;
/// `authority` must be the mint authority. Used to fund the leader's ATAs
/// before the bootstrap's seed `deposit_leader`.
pub fn mint_to(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    ata: &Pubkey,
    amount: u64,
) -> Result<String> {
    // SPL Token `MintTo` (index 7): the u64 amount.
    let mut data = vec![7u8];
    data.extend_from_slice(&amount.to_le_bytes());
    let ix = Instruction::new_with_bytes(
        SPL_TOKEN_PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new(*mint, false),
            AccountMeta::new(*ata, false),
            AccountMeta::new_readonly(authority.pubkey(), true),
        ],
    );
    send(client, authority, &[authority], &[ix])
}

/// Create the associated token account for `(wallet, mint, SPL Token)`
/// idempotently (ATA program `CreateIdempotent`, index 1), paid by `payer`.
/// Returns the ATA address. Idempotent so a re-run after a partial bootstrap
/// doesn't fail on an ATA that already exists.
pub fn create_ata_idempotent(
    client: &RpcClient,
    payer: &Keypair,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey> {
    let ata = associated_token_address(wallet, mint, &SPL_TOKEN_PROGRAM_ID);
    let ix = Instruction::new_with_bytes(
        ATA_PROGRAM_ID,
        &[1u8],
        vec![
            AccountMeta::new(payer.pubkey(), true),
            AccountMeta::new(ata, false),
            AccountMeta::new_readonly(*wallet, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(SPL_TOKEN_PROGRAM_ID, false),
        ],
    );
    send(client, payer, &[payer], &[ix]).context("create ATA")?;
    Ok(ata)
}

/// A System-program `transfer` of `lamports` from `from` (signer) to `to`.
/// Used to top up a freshly-`init`'d PDA over the rent floor — see
/// [`RENT_TOPUP_LAMPORTS`].
pub fn system_transfer_ix(from: &Pubkey, to: &Pubkey, lamports: u64) -> Instruction {
    let mut data = Vec::with_capacity(12);
    // System instruction index 2 = Transfer, then the u64 lamports.
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction::new_with_bytes(
        SYSTEM_PROGRAM_ID,
        &data,
        vec![AccountMeta::new(*from, true), AccountMeta::new(*to, false)],
    )
}

/// Request `lamports` from the validator faucet for `to` and block until
/// the airdrop confirms (or time out after ~10s). Localnet only.
pub fn airdrop(client: &RpcClient, to: &Pubkey, lamports: u64) -> Result<()> {
    let sig = client
        .request_airdrop(to, lamports)
        .context("request airdrop")?;
    for _ in 0..50 {
        if client.confirm_transaction(&sig).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    anyhow::bail!("airdrop did not confirm in time")
}

/// Sign `ixs` with `signers` (fee payer = `payer`) and send, confirming at
/// the client's commitment. Returns the transaction signature as a string.
///
/// On failure the error carries the program-log stream: a
/// `ClientError`'s `Display` drops the logs for a custom-program-error, so
/// we re-simulate the (already-signed) transaction to recover them — state
/// is unchanged after a failed send, so the simulation reproduces the same
/// error and logs.
pub fn send(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Result<String> {
    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    match client.send_and_confirm_transaction(&tx) {
        Ok(sig) => Ok(sig.to_string()),
        Err(err) => {
            let logs = client
                .simulate_transaction(&tx)
                .ok()
                .and_then(|r| r.value.logs)
                .filter(|l| !l.is_empty())
                .map(|l| format!("\n{}", l.join("\n")))
                .unwrap_or_default();
            Err(anyhow::anyhow!("{err}{logs}"))
        }
    }
}

/// Like [`send`], but also reports the transaction's compute-unit cost — the
/// value the CU pane watches. The CU is read by simulating the (signed)
/// transaction against the same pre-state it will execute against, which is
/// exact for these deterministic instructions and needs no transaction-history
/// RPC. A `None` CU (simulation unavailable) still sends and returns the
/// signature, so a transient simulate failure never blocks the operation.
pub fn send_measured(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Result<(String, Option<u64>)> {
    let cu = measure_cu(client, payer, signers, ixs);
    let sig = send(client, payer, signers, ixs)?;
    Ok((sig, cu))
}

/// Send `ixs` as one transaction, log its signature under `label`, route the
/// measured CU to the CU pane (when available), and return the signature. The
/// one-stop send for operations whose per-instruction cost the TUI surfaces.
pub fn send_logged(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
    label: &str,
    log: &Logger,
) -> Result<String> {
    let (sig, cu) = send_measured(client, payer, signers, ixs)?;
    log.log(format!("{label}: {sig}"));
    if let Some(units) = cu {
        log.cu(label.to_string(), units);
    }
    Ok(sig)
}

/// Simulate `ixs` (signed, against current state) purely to recover the
/// compute units consumed. Best-effort — any RPC hiccup yields `None`.
fn measure_cu(
    client: &RpcClient,
    payer: &Keypair,
    signers: &[&Keypair],
    ixs: &[Instruction],
) -> Option<u64> {
    let blockhash = client.get_latest_blockhash().ok()?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    client.simulate_transaction(&tx).ok()?.value.units_consumed
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
        let fee_source = Pubkey::new_unique();
        let base = Pubkey::new_unique();
        let quote = Pubkey::new_unique();
        let fee_mint = Pubkey::new_unique();
        let registry = registry_pda();
        let market = market_pda(&base, &quote);
        let ix = build_create_market_ix(
            &payer,
            &fee_source,
            &base,
            &quote,
            &fee_mint,
            &SPL_TOKEN_PROGRAM_ID,
        );
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
                (fee_source, false, true),
                (
                    associated_token_address(&registry, &fee_mint, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (SYSTEM_PROGRAM_ID, false, false),
                (ATA_PROGRAM_ID, false, false),
            ]
        );
        // The fee source must not alias the payer — anchor-v2 rejects the
        // same key in two mutable slots.
        assert_ne!(payer, fee_source);
    }

    /// Pins the `create_vault` ordering (admin path) against
    /// `fixture::create_vault_meta`, including the trailing
    /// event_authority · program self-CPI pair.
    #[test]
    fn create_vault_ordering_matches_fixture() {
        let payer = Pubkey::new_unique();
        let fee_source = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let fee_mint = Pubkey::new_unique();
        let leader = Pubkey::new_unique();
        let registry = registry_pda();
        let ix = build_create_vault_ix(
            &payer,
            &fee_source,
            &market,
            &fee_mint,
            &SPL_TOKEN_PROGRAM_ID,
            &leader,
        );
        assert_eq!(
            metas(&ix),
            vec![
                (payer, true, true),
                (registry, false, false),
                (market, false, true),
                (fee_mint, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (fee_source, false, true),
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
        // Neither the fee source nor the leader may alias the admin payer.
        assert_ne!(payer, fee_source);
        assert_ne!(payer, leader);
    }

    /// Pins the `deposit_leader` ordering against `fixture`'s
    /// `deposit_leader_as_meta` ix: leader(signer,w) · market(w) ·
    /// base_mint · quote_mint · base_tp · quote_tp · leader_base(w) ·
    /// leader_quote(w) · base_treasury(w) · quote_treasury(w) · system ·
    /// ata · event_authority · program — so a transposed base/quote leg or
    /// ATA/treasury slot is caught at `cargo test`.
    #[test]
    fn deposit_leader_ordering_matches_fixture() {
        let leader = Pubkey::new_unique();
        let market = Pubkey::new_unique();
        let base_mint = Pubkey::new_unique();
        let quote_mint = Pubkey::new_unique();
        let base_treasury = Pubkey::new_unique();
        let quote_treasury = Pubkey::new_unique();
        let ix = build_deposit_leader_ix(
            &leader,
            &market,
            &base_mint,
            &quote_mint,
            &base_treasury,
            &quote_treasury,
            0,
            1_000,
            2_000,
        );
        assert_eq!(
            metas(&ix),
            vec![
                (leader, true, true),
                (market, false, true),
                (base_mint, false, false),
                (quote_mint, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (SPL_TOKEN_PROGRAM_ID, false, false),
                (
                    associated_token_address(&leader, &base_mint, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (
                    associated_token_address(&leader, &quote_mint, &SPL_TOKEN_PROGRAM_ID),
                    false,
                    true
                ),
                (base_treasury, false, true),
                (quote_treasury, false, true),
                (SYSTEM_PROGRAM_ID, false, false),
                (ATA_PROGRAM_ID, false, false),
                (event_authority(), false, false),
                (DROPSET_ID, false, false),
            ]
        );
    }
}
