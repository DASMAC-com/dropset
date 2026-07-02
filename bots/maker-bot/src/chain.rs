//! On-chain I/O — market discovery, the live vault read, and the two
//! quoting-path sends.
//!
//! Discovery mirrors the TUI (`tui/src/accounts.rs`): scan the program's
//! accounts for the `MarketHeader` discriminator, decode the single localnet
//! market through the slab-layout mirror, and read the pair's mint decimals so
//! inventory can be valued. The instruction builders are the SDK's `quoting`
//! helpers, signed and paid by the leader (its quote-authority is what gates
//! the hot/cold path); on localnet the leader airdrops its own fees.

use anyhow::{anyhow, Context as _, Result};
use dropset_sdk::accounts::MARKET_HEADER_DISCRIMINATOR;
use dropset_sdk::layout::MarketView as SlabView;
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{set_liquidity_profile_ix, set_reference_price_ix};
use dropset_sdk::DROPSET_ID;
use solana_client::rpc_client::RpcClient;
use solana_commitment_config::CommitmentConfig;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use std::time::Duration;

use crate::context::{MarketAddrs, VaultSnapshot};

/// Decode scale for a `Price` to a float — `value × 10^9`, matching the SDK's
/// `quoting` module.
const PRICE_SCALE: u64 = 1_000_000_000;

/// Convert a human quote-per-base price (USD per token, as the feeds report it)
/// into the **atoms-ratio** the on-chain `Price` encodes — `quote_atoms` per
/// `base_atoms`. They coincide only when both legs share decimals (an
/// equal-decimals market stamps the human price directly); a token with more
/// decimals than USDC scales down, fewer scales up. This is the per-market
/// decimal handling the wide-unit-price roster (EURC ~$1.14 … IDRX ~$0.000056)
/// needs so each market's reference encodes correctly.
pub fn human_to_atoms_ratio(human: f64, base_decimals: u8, quote_decimals: u8) -> f64 {
    human * 10f64.powi(quote_decimals as i32 - base_decimals as i32)
}

/// Inverse of [`human_to_atoms_ratio`] — decode an on-chain atoms-ratio back to
/// the human quote-per-base price for display.
pub fn atoms_ratio_to_human(ratio: f64, base_decimals: u8, quote_decimals: u8) -> f64 {
    ratio * 10f64.powi(base_decimals as i32 - quote_decimals as i32)
}

/// SPL Token Mint `decimals` byte offset (after `COption<Pubkey>` authority +
/// `u64` supply).
const MINT_DECIMALS_OFFSET: usize = 44;

/// An `RpcClient` at `confirmed`, pointed at `url`.
pub fn rpc(url: &str) -> RpcClient {
    RpcClient::new_with_timeout_and_commitment(
        url.to_string(),
        Duration::from_secs(10),
        CommitmentConfig::confirmed(),
    )
}

/// The genesis hashes of the three public Solana clusters. `assert_localnet`
/// refuses to run against any of them — the airdrop needs the localnet faucet
/// and the leader key holds no authority on a public cluster, so running
/// off-localnet is always a misconfiguration. Cross-checked against the Solana
/// docs and the gill / mpl-bubblegum SDKs.
const MAINNET_GENESIS: &str = "5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d";
const DEVNET_GENESIS: &str = "EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG";
const TESTNET_GENESIS: &str = "4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY";

/// The name of the public Solana cluster with this genesis hash, or `None` for
/// any other cluster (a localnet test validator mints a fresh genesis per
/// launch). Pure, so the denylist is unit-testable without a validator.
fn public_cluster(genesis: &str) -> Option<&'static str> {
    match genesis {
        MAINNET_GENESIS => Some("mainnet-beta"),
        DEVNET_GENESIS => Some("devnet"),
        TESTNET_GENESIS => Some("testnet"),
        _ => None,
    }
}

/// Abort unless `client` is a localnet validator. Keyed on the cluster's
/// genesis hash rather than the RPC host, so it allows a localnet on any
/// address (LAN, Docker) yet still trips on a port-forward / proxy that tunnels
/// a public cluster through a loopback URL. Call once at startup, before the
/// first signed send.
pub fn assert_localnet(client: &RpcClient) -> Result<()> {
    let genesis = client
        .get_genesis_hash()
        .context("get genesis hash")?
        .to_string();
    if let Some(cluster) = public_cluster(&genesis) {
        return Err(anyhow!(
            "refusing to run against the {cluster} public cluster (genesis \
             {genesis}): this localnet bot signs quoting transactions with the \
             leader key and must run only against a localnet test validator"
        ));
    }
    Ok(())
}

/// Airdrop `lamports` to `who` and block until it confirms (localnet faucet).
/// Used to fund the leader's fees, since it pays for its own quoting txns.
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

/// Discover every localnet market in one scan of the program's accounts for the
/// `MarketHeader` discriminator, reading each one's mints, treasuries, and the
/// pair's decimals. The supervisor matches these against the [`MARKETS`] roster
/// by base mint to find each bot's market.
///
/// [`MARKETS`]: crate::config::MARKETS
pub fn discover_markets(client: &RpcClient) -> Result<Vec<MarketAddrs>> {
    let accounts = client
        .get_program_accounts(&DROPSET_ID)
        .context("get_program_accounts")?;
    let mut markets = Vec::new();
    for (address, account) in &accounts {
        if account.data.len() < 8 || account.data[..8] != MARKET_HEADER_DISCRIMINATOR {
            continue;
        }
        // Skip (don't abort the whole scan on) an account that carries the
        // header discriminator but won't decode — one malformed market must not
        // hide the rest of the roster.
        let view = match SlabView::load(&account.data) {
            Ok(view) => view,
            Err(e) => {
                eprintln!("[discover] skipping undecodable market {address}: {e:?}");
                continue;
            }
        };
        let header = view.header;
        let base_mint = Pubkey::new_from_array(header.base_mint);
        let quote_mint = Pubkey::new_from_array(header.quote_mint);
        markets.push(MarketAddrs {
            market: *address,
            base_mint,
            quote_mint,
            base_treasury: Pubkey::new_from_array(header.base_treasury),
            quote_treasury: Pubkey::new_from_array(header.quote_treasury),
            base_decimals: mint_decimals(client, &base_mint).context("base mint decimals")?,
            quote_decimals: mint_decimals(client, &quote_mint).context("quote mint decimals")?,
        });
    }
    Ok(markets)
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

/// Read the bot's vault — the active sector whose quote authority is
/// `authority` (the leader). Matching by authority rather than a hardcoded
/// sector index makes the bot robust to whichever sector the bootstrap
/// happened to open. Reports the active sectors on a miss.
///
/// Note the reference's price-time nonce is deliberately *not* read for fill
/// detection: it bumps on every re-quote (the leader's own
/// `set_reference_price` / `set_liquidity_profile` arm a flush), so a change
/// doesn't imply a taker. The `emit_cpi!` `FillEvent` subscription (`fills`)
/// is the primary fill signal; this read reconciles it and is the fallback.
pub fn read_vault(
    client: &RpcClient,
    market: &Pubkey,
    authority: &Pubkey,
    base_decimals: u8,
    quote_decimals: u8,
) -> Result<VaultSnapshot> {
    let account = client.get_account(market).context("get market account")?;
    let view = SlabView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;

    let wanted = authority.to_bytes();
    let mut active = Vec::new();
    for (idx, vault) in view.active_vaults() {
        active.push(idx);
        if vault.quote_authority == wanted {
            let reference = vault.reference_price.price();
            let ratio = reference.quote_for_base(PRICE_SCALE) as f64 / PRICE_SCALE as f64;
            // The stored price is the atoms-ratio; lift it back to the human
            // quote-per-base for the snapshot.
            let reference_price = atoms_ratio_to_human(ratio, base_decimals, quote_decimals);
            return Ok(VaultSnapshot {
                sector_idx: idx,
                base_atoms: vault.base_atoms.get(),
                quote_atoms: vault.quote_atoms.get(),
                reference_price,
                frozen: vault.frozen != 0,
            });
        }
    }
    Err(anyhow!(
        "no vault with quote authority {authority}; active sectors: {active:?}"
    ))
}

/// Stamp a new reference price (`set_reference_price`, hot path). `slot` is the
/// quote slot; it is not backdated on this path (§3 heartbeat invariant).
#[allow(clippy::too_many_arguments)]
pub fn set_reference_price(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    price: f64,
    base_decimals: u8,
    quote_decimals: u8,
    slot: u64,
) -> Result<String> {
    // The feeds report a human quote-per-base price; the engine stores the
    // atoms-ratio, so scale by the decimal gap before encoding.
    let ratio = human_to_atoms_ratio(price, base_decimals, quote_decimals);
    let reference = Price::from_value(ratio)
        .ok_or_else(|| anyhow!("price {price} (ratio {ratio}) out of range"))?;
    let ix = set_reference_price_ix(leader.pubkey(), *market, vault_idx, reference, slot);
    send(client, leader, &[ix])
}

/// Rewrite the quote ladder (`set_liquidity_profile`, cold path).
pub fn set_liquidity_profile(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    profile_bytes: [u8; 160],
) -> Result<String> {
    let ix = set_liquidity_profile_ix(leader.pubkey(), *market, vault_idx, profile_bytes);
    send(client, leader, &[ix])
}

/// Current slot, for stamping the reference's `quote_slot`.
pub fn current_slot(client: &RpcClient) -> Result<u64> {
    client.get_slot().context("get_slot")
}

/// Sign `ixs` with the leader (fee payer and only signer) and send,
/// confirming at the client's commitment. On failure, re-simulate to recover
/// the program logs a `ClientError` drops for a custom-program error.
fn send(client: &RpcClient, leader: &Keypair, ixs: &[Instruction]) -> Result<String> {
    let blockhash = client.get_latest_blockhash().context("blockhash")?;
    let tx = Transaction::new_signed_with_payer(ixs, Some(&leader.pubkey()), &[leader], blockhash);
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

    /// The public clusters are named (and so rejected); any other genesis — a
    /// fresh test-validator's — reads as localnet and passes.
    #[test]
    fn public_clusters_are_named_localnet_passes() {
        assert_eq!(public_cluster(MAINNET_GENESIS), Some("mainnet-beta"));
        assert_eq!(public_cluster(DEVNET_GENESIS), Some("devnet"));
        assert_eq!(public_cluster(TESTNET_GENESIS), Some("testnet"));
        assert_eq!(public_cluster("11111111111111111111111111111111"), None);
    }

    #[test]
    fn atoms_ratio_is_identity_at_equal_decimals() {
        // EURC (6) / USDC (6): the human price stamps unchanged.
        assert!((human_to_atoms_ratio(1.14, 6, 6) - 1.14).abs() < 1e-12);
    }

    #[test]
    fn atoms_ratio_scales_with_the_decimal_gap() {
        // VCHF (9) / USDC (6): 1 VCHF-atom is 10^-3 of a token, so the
        // atoms-ratio is the human price × 10^(6-9).
        assert!((human_to_atoms_ratio(1.235, 9, 6) - 1.235e-3).abs() < 1e-12);
        // IDRX (2) / USDC (6): the atoms-ratio scales up.
        assert!((human_to_atoms_ratio(0.000056, 2, 6) - 0.56).abs() < 1e-12);
    }

    #[test]
    fn atoms_ratio_round_trips_to_human() {
        for (human, base, quote) in [(1.14, 6, 6), (1.235, 9, 6), (0.000056, 2, 6)] {
            let ratio = human_to_atoms_ratio(human, base, quote);
            let back = atoms_ratio_to_human(ratio, base, quote);
            assert!((back - human).abs() / human < 1e-12, "round-trip {human}");
        }
    }
}
