//! On-chain I/O — market discovery, self-funding, off-chain order sizing, and
//! the swap send.
//!
//! Discovery mirrors the maker-bot (`bots/maker-bot/src/chain.rs`): scan the
//! program's accounts for the `MarketHeader` discriminator and decode the
//! single localnet market through the slab-layout mirror. Funding mirrors the
//! TUI's localnet plumbing (`tui/src/chain.rs`): airdrop the taker its fee
//! SOL, create its two ATAs, and mint it starting inventory under the mock
//! mints' authority. Each order is **sized off-chain** before it is sent:
//! [`dropset_sdk::matching::simulate_swap`] turns a sampled notional into the
//! achievable `amount_in` / `min_out` at the live book, and the swap itself is
//! built with the generated [`SwapBuilder`] and signed by the taker.

// cspell:word idempotently

use anyhow::{anyhow, Context as _, Result};
use dropset_sdk::accounts::MARKET_HEADER_DISCRIMINATOR;
use dropset_sdk::instructions::SwapBuilder;
use dropset_sdk::layout::MarketView;
use dropset_sdk::matching::{simulate_swap, SwapSide};
use dropset_sdk::price::Price;
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::time::Duration;

use crate::context::MarketAddrs;
use crate::model::Order;

/// SPL Token program (the mock CADC/USDC mints live here, not Token-2022).
pub const SPL_TOKEN_PROGRAM_ID: Pubkey = pubkey!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
/// Associated Token Account program.
pub const ATA_PROGRAM_ID: Pubkey = pubkey!("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
/// System program.
pub const SYSTEM_PROGRAM_ID: Pubkey = pubkey!("11111111111111111111111111111111");

/// SPL Token Mint `decimals` byte offset (after `COption<Pubkey>` authority +
/// `u64` supply).
const MINT_DECIMALS_OFFSET: usize = 44;
/// SPL Token Account `amount` (`u64`) byte offset (after mint + owner).
const TOKEN_ACCOUNT_AMOUNT_OFFSET: usize = 64;

/// An `RpcClient` at `confirmed`, pointed at `url`.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        url.to_string(),
        Duration::from_secs(10),
        CommitmentConfig::confirmed(),
    )
}

/// The self-CPI event-authority PDA — seeds `[b"__event_authority"]`.
fn event_authority() -> Pubkey {
    Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID).0
}

/// Canonical associated-token-account address for `(wallet, mint)` under the
/// SPL Token program — seeds `[wallet, token_program, mint]`.
pub fn associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[
            wallet.as_ref(),
            SPL_TOKEN_PROGRAM_ID.as_ref(),
            mint.as_ref(),
        ],
        &ATA_PROGRAM_ID,
    )
    .0
}

/// Airdrop `lamports` to `who` and block until it confirms (localnet faucet).
pub fn airdrop(client: &RpcClient, who: &Pubkey, lamports: u64) -> Result<()> {
    let sig = client.request_airdrop(who, lamports).context("airdrop")?;
    for _ in 0..50 {
        if client.confirm_transaction(&sig).unwrap_or(false) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(anyhow!("airdrop did not confirm in time"))
}

/// Discover the single localnet market by scanning the program's accounts for
/// the `MarketHeader` discriminator, then read its mints, treasuries, and the
/// pair's decimals.
pub fn discover_market(client: &RpcClient) -> Result<MarketAddrs> {
    let accounts = client
        .get_program_accounts(&DROPSET_ID)
        .context("get_program_accounts")?;
    let (address, account) = accounts
        .iter()
        .find(|(_, a)| a.data.len() >= 8 && a.data[..8] == MARKET_HEADER_DISCRIMINATOR)
        .ok_or_else(|| anyhow!("no market found — is the localnet bootstrapped?"))?;

    let view = MarketView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;
    let header = view.header;
    let base_mint = Pubkey::new_from_array(header.base_mint);
    let quote_mint = Pubkey::new_from_array(header.quote_mint);

    Ok(MarketAddrs {
        market: *address,
        base_mint,
        quote_mint,
        base_treasury: Pubkey::new_from_array(header.base_treasury),
        quote_treasury: Pubkey::new_from_array(header.quote_treasury),
        base_decimals: mint_decimals(client, &base_mint).context("base mint decimals")?,
        quote_decimals: mint_decimals(client, &quote_mint).context("quote mint decimals")?,
    })
}

/// Read an SPL mint's `decimals`.
fn mint_decimals(client: &RpcClient, mint: &Pubkey) -> Result<u8> {
    let account = client.get_account(mint).context("get mint account")?;
    account
        .data
        .get(MINT_DECIMALS_OFFSET)
        .copied()
        .ok_or_else(|| anyhow!("mint account too small"))
}

/// Read an SPL token account's `amount` (atoms), or `0` if it doesn't exist
/// yet (an un-created ATA holds nothing).
fn token_balance(client: &RpcClient, ata: &Pubkey) -> Result<u64> {
    let Some(account) = client
        .get_account_with_commitment(ata, client.commitment())?
        .value
    else {
        return Ok(0);
    };
    let bytes = account
        .data
        .get(TOKEN_ACCOUNT_AMOUNT_OFFSET..TOKEN_ACCOUNT_AMOUNT_OFFSET + 8)
        .ok_or_else(|| anyhow!("token account too small"))?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

/// Create the associated token account for `(wallet, mint)` idempotently (ATA
/// program `CreateIdempotent`, index 1), paid by `payer`. Returns the ATA.
pub fn create_ata_idempotent(
    client: &RpcClient,
    payer: &Keypair,
    wallet: &Pubkey,
    mint: &Pubkey,
) -> Result<Pubkey> {
    let ata = associated_token_address(wallet, mint);
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

/// Mint `amount` atoms of `mint` to `ata`; `authority` must be the mint
/// authority (SPL Token `MintTo`, index 7).
pub fn mint_to(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    ata: &Pubkey,
    amount: u64,
) -> Result<String> {
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

/// Whole-token count `tokens` expressed in atoms for `decimals`.
fn to_atoms(tokens: f64, decimals: u8) -> u64 {
    (tokens * 10f64.powi(decimals as i32)) as u64
}

/// Ensure the taker can trade: top up its SOL when low, create its two ATAs,
/// and refill either leg that has fallen below `min_tokens` back up to
/// `target_tokens` (minted under the mock-mint authority). Idempotent, so it
/// is safe to call every tick.
#[allow(clippy::too_many_arguments)]
pub fn ensure_funded(
    client: &RpcClient,
    taker: &Keypair,
    mint_authority: &Keypair,
    market: &MarketAddrs,
    airdrop_lamports: u64,
    min_lamports: u64,
    target_tokens: f64,
    min_tokens: f64,
) -> Result<()> {
    let balance = client
        .get_balance(&taker.pubkey())
        .context("taker balance")?;
    if balance < min_lamports {
        airdrop(client, &taker.pubkey(), airdrop_lamports)?;
    }

    let base_ata = create_ata_idempotent(client, taker, &taker.pubkey(), &market.base_mint)
        .context("taker base ATA")?;
    let quote_ata = create_ata_idempotent(client, taker, &taker.pubkey(), &market.quote_mint)
        .context("taker quote ATA")?;

    refill_leg(
        client,
        mint_authority,
        &market.base_mint,
        &base_ata,
        market.base_decimals,
        target_tokens,
        min_tokens,
    )
    .context("refill base leg")?;
    refill_leg(
        client,
        mint_authority,
        &market.quote_mint,
        &quote_ata,
        market.quote_decimals,
        target_tokens,
        min_tokens,
    )
    .context("refill quote leg")?;
    Ok(())
}

/// Mint `ata` back up to `target_tokens` when its balance is below
/// `min_tokens`.
fn refill_leg(
    client: &RpcClient,
    authority: &Keypair,
    mint: &Pubkey,
    ata: &Pubkey,
    decimals: u8,
    target_tokens: f64,
    min_tokens: f64,
) -> Result<()> {
    let balance = token_balance(client, ata)?;
    let min_atoms = to_atoms(min_tokens, decimals);
    if balance >= min_atoms {
        return Ok(());
    }
    let target_atoms = to_atoms(target_tokens, decimals);
    let deficit = target_atoms.saturating_sub(balance);
    if deficit > 0 {
        mint_to(client, authority, mint, ata, deficit)?;
    }
    Ok(())
}

/// A swap sized against the live book, ready to submit.
#[derive(Clone, Copy, Debug)]
pub struct SizedSwap {
    pub side: SwapSide,
    /// Exact input atoms (quote for a Buy, base for a Sell).
    pub amount_in: u64,
    /// Worst acceptable fill price, encoded `Price` bits.
    pub limit_price_bits: u32,
    /// Slippage floor on the output leg (atoms).
    pub min_out: u64,
    /// The simulator's expected net output at the limit (atoms) — for logging.
    pub expected_out: u64,
}

/// The market's current reference price (quote-per-base) as a float, taken
/// from the first active, validly-priced vault. `None` if no vault is quoting.
fn market_reference_price(view: &MarketView<'_>) -> Option<f64> {
    view.active_vaults().find_map(|(_, v)| {
        let p = v.reference_price.price();
        (p.is_valid() && !p.is_zero() && !p.is_infinity()).then(|| p.to_f64())
    })
}

/// Size one sampled [`Order`] against the live book: convert its quote
/// notional into an `amount_in` for the chosen leg, derive the
/// `limit_price_bits` from the reference price and slippage tolerance, and
/// floor `min_out` below what the simulator says fills within that bound.
///
/// Returns `None` when the order can't be priced or wouldn't fill — no quoting
/// vault, a zero-atom size, an out-of-range limit price, or no liquidity
/// inside the bound — so the tick simply skips it.
pub fn size_order(
    client: &RpcClient,
    market: &MarketAddrs,
    order: &Order,
    slippage: f64,
) -> Result<Option<SizedSwap>> {
    let account = client
        .get_account(&market.market)
        .context("get market account")?;
    let view = MarketView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;
    let slot = client.get_slot().context("get_slot")? as u32;

    let Some(price) = market_reference_price(&view) else {
        return Ok(None);
    };

    // Convert the quote notional into the input leg's atoms.
    let amount_in = match order.side {
        SwapSide::Buy => to_atoms(order.notional, market.quote_decimals),
        SwapSide::Sell => to_atoms(order.notional / price, market.base_decimals),
    };
    if amount_in == 0 {
        return Ok(None);
    }

    // Worst acceptable price: above the reference for a Buy, below for a Sell.
    let limit_value = match order.side {
        SwapSide::Buy => price * (1.0 + slippage),
        SwapSide::Sell => price * (1.0 - slippage),
    };
    let Some(limit_price) = Price::from_value(limit_value) else {
        return Ok(None);
    };

    let quote = simulate_swap(&view, order.side, amount_in, limit_price, slot);
    if quote.out_amount == 0 {
        return Ok(None);
    }
    // Floor `min_out` below the simulated output so a benign book move between
    // sizing and execution doesn't trip the on-chain slippage check.
    let min_out = (quote.out_amount as f64 * (1.0 - slippage)) as u64;

    Ok(Some(SizedSwap {
        side: order.side,
        amount_in,
        limit_price_bits: limit_price.as_u32(),
        min_out,
        expected_out: quote.out_amount,
    }))
}

/// Build and send a `swap`, signed and paid by the taker. Returns the
/// transaction signature.
pub fn send_swap(
    client: &RpcClient,
    taker: &Keypair,
    market: &MarketAddrs,
    swap: &SizedSwap,
) -> Result<String> {
    let ix = SwapBuilder::new()
        .taker(taker.pubkey())
        .market(market.market)
        .base_mint(market.base_mint)
        .quote_mint(market.quote_mint)
        .base_token_program(SPL_TOKEN_PROGRAM_ID)
        .quote_token_program(SPL_TOKEN_PROGRAM_ID)
        .taker_base_ata(associated_token_address(&taker.pubkey(), &market.base_mint))
        .taker_quote_ata(associated_token_address(
            &taker.pubkey(),
            &market.quote_mint,
        ))
        .market_base_treasury(market.base_treasury)
        .market_quote_treasury(market.quote_treasury)
        .event_authority(event_authority())
        .program(DROPSET_ID)
        .side(swap.side as u8)
        .amount_in(swap.amount_in)
        .limit_price_bits(swap.limit_price_bits)
        .min_out(swap.min_out)
        .instruction();
    send(client, taker, &[taker], &[ix])
}

/// Sign `ixs` with `signers` (fee payer = `payer`) and send, confirming at the
/// client's commitment. On failure, re-simulate to recover the program logs a
/// `ClientError` drops for a custom-program error (state is unchanged after a
/// failed send, so the re-simulation reproduces the same error).
fn send(
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
            Err(anyhow!("{err}{logs}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Atom conversion respects decimals and truncates toward zero.
    #[test]
    fn to_atoms_scales_by_decimals() {
        assert_eq!(to_atoms(1.0, 6), 1_000_000);
        assert_eq!(to_atoms(0.73, 6), 730_000);
        assert_eq!(to_atoms(2.5, 0), 2);
    }

    /// The taker ATA derivation matches the canonical seed order.
    #[test]
    fn ata_is_canonical() {
        let wallet = Pubkey::new_unique();
        let mint = Pubkey::new_unique();
        let expected = Pubkey::find_program_address(
            &[
                wallet.as_ref(),
                SPL_TOKEN_PROGRAM_ID.as_ref(),
                mint.as_ref(),
            ],
            &ATA_PROGRAM_ID,
        )
        .0;
        assert_eq!(associated_token_address(&wallet, &mint), expected);
    }

    /// The event-authority PDA matches the program's `[b"__event_authority"]`
    /// seed (the same one the SDK's own adapters derive).
    #[test]
    fn event_authority_is_canonical() {
        assert_eq!(
            event_authority(),
            Pubkey::find_program_address(&[b"__event_authority"], &DROPSET_ID).0
        );
    }
}
