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

    let view = SlabView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;
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
) -> Result<VaultSnapshot> {
    let account = client.get_account(market).context("get market account")?;
    let view = SlabView::load(&account.data).map_err(|e| anyhow!("decode market: {e:?}"))?;

    let wanted = authority.to_bytes();
    let mut active = Vec::new();
    for (idx, vault) in view.active_vaults() {
        active.push(idx);
        if vault.quote_authority == wanted {
            let reference = vault.reference_price.price();
            let reference_price = reference.quote_for_base(PRICE_SCALE) as f64 / PRICE_SCALE as f64;
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
pub fn set_reference_price(
    client: &RpcClient,
    leader: &Keypair,
    market: &Pubkey,
    vault_idx: u32,
    price: f64,
    slot: u64,
) -> Result<String> {
    let reference =
        Price::from_value(price).ok_or_else(|| anyhow!("price {price} out of range"))?;
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
}
