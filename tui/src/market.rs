//! Reusable, config-driven market bootstrap.
//!
//! The TUI's market setup used to mint a throwaway random base/quote pair
//! on every run, so each bootstrap produced a *different* market address —
//! fine for driving the TUI alone, useless for anything that needs a stable
//! target (a market-making bot, the explorer). This module generalizes it:
//! a [`PairConfig`] names two **fixed, checked-in** mint keypairs (so the
//! market PDA, seeded on `[base, quote]`, is the same address every run), a
//! leader role key, and the reference price / quote ladder / seed deposit
//! that bring the vault up fully quotable and seeded. Drop in another pair
//! by adding another `PairConfig`; nothing else changes.
//!
//! The localnet keys live under `keys/` (see `keys/README.md`); their paths
//! resolve against the repo root the TUI already locates. The admin wallet
//! is the fee payer and mint authority for every transaction here; the
//! leader co-signs only the vault-gated instructions (`set_reference_price`,
//! `set_liquidity_profile`, `deposit_leader`), so it needs no SOL balance.

// cspell:word keypairs

use crate::accounts::MarketView;
use crate::chain;
use crate::job::Logger;
use anyhow::{Context, Result};
use bytemuck::Zeroable;
use dropset_sdk::layout::LiquidityProfile;
use dropset_sdk::price::Price;
use dropset_sdk::quoting::{profile_bytes, set_liquidity_profile_ix, set_reference_price_ix};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::path::Path;

/// A mock SPL mint in a localnet pair: a checked-in keypair (named relative
/// to the repo root), a human symbol for the log, and its decimals.
pub struct MintSpec {
    pub symbol: &'static str,
    pub keypair_file: &'static str,
    pub decimals: u8,
}

/// A localnet market pair plus everything the bootstrap needs to bring it
/// up quotable and seeded: the two mints, the leader role key that quotes
/// and seeds the vault, the reference price, a symmetric quote ladder, and
/// the leader's opening deposit. One per tradeable pair.
pub struct PairConfig {
    pub base: MintSpec,
    pub quote: MintSpec,
    /// Leader + quote-authority keypair (a checked-in role key). Must not be
    /// the admin wallet — anchor-v2 rejects the same key in the admin and
    /// leader slots of `create_vault`.
    pub leader_keypair_file: &'static str,
    /// Reference (quote-per-base) price, e.g. `0.73` USDC per CADC.
    pub reference_price: f64,
    /// Symmetric quote: a `±offset_ppm` spread around the reference, each
    /// side sized `size_bps` of its inventory leg, expiring `expiry_offset`
    /// slots after the quote.
    pub offset_ppm: u32,
    pub size_bps: u16,
    pub expiry_offset: u32,
    /// The leader's opening deposit, `(base_atoms, quote_atoms)`.
    pub leader_deposit: (u64, u64),
}

/// The mock CADC/USDC pair the localnet bootstrap brings up by default:
/// base CADC, quote USDC, both 6-decimal, led by the `EEEE` role key,
/// anchored at 0.73 USDC/CADC with a full-inventory ±0.5% ladder and an
/// inventory balanced at that reference (1,000,000 CADC / 730,000 USDC).
pub const MOCK_CADC_USDC: PairConfig = PairConfig {
    base: MintSpec {
        symbol: "CADC",
        keypair_file: "keys/CADC.json",
        decimals: 6,
    },
    quote: MintSpec {
        symbol: "USDC",
        keypair_file: "keys/USDC.json",
        decimals: 6,
    },
    leader_keypair_file: "keys/EEEE.json",
    reference_price: 0.73,
    offset_ppm: 5_000,       // ±0.5%
    size_bps: 10_000,        // 100% of each leg
    expiry_offset: u32::MAX, // never expires
    leader_deposit: (1_000_000_000_000, 730_000_000_000),
};

/// The bootstrap always opens the market's first vault, so its sector index
/// is 0. The seed instructions address the vault by this index.
const VAULT_IDX: u32 = 0;

/// Load the leader / quote-authority keypair `config` names.
pub fn leader(repo_root: &Path, config: &PairConfig) -> Result<Keypair> {
    load_key(repo_root, config.leader_keypair_file)
}

/// Create `config`'s two fixed mints at their checked-in addresses, with the
/// admin `wallet` as mint authority, and return `(base_mint, quote_mint)`.
pub fn create_pair_mints(
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    config: &PairConfig,
    log: &Logger,
) -> Result<(Pubkey, Pubkey)> {
    let base = load_key(repo_root, config.base.keypair_file)?;
    let quote = load_key(repo_root, config.quote.keypair_file)?;
    log.log(format!(
        "Creating fixed {} + {} mints…",
        config.base.symbol, config.quote.symbol
    ));
    chain::create_mint(client, wallet, &base, config.base.decimals).context("create base mint")?;
    chain::create_mint(client, wallet, &quote, config.quote.decimals)
        .context("create quote mint")?;
    log.log(format!("{}: {}", config.base.symbol, base.pubkey()));
    log.log(format!("{}: {}", config.quote.symbol, quote.pubkey()));
    Ok((base.pubkey(), quote.pubkey()))
}

/// Bring the market's freshly-created vault up live: stamp the reference
/// price, set the quote ladder, then fund the leader's ATAs (from the admin
/// mint authority) and seed the vault with `deposit_leader`. The `leader`
/// must be `config`'s leader key — it co-signs each instruction as the
/// vault's quote authority / leader, while `wallet` (admin) pays the fees.
pub fn seed_vault(
    client: &RpcClient,
    wallet: &Keypair,
    leader: &Keypair,
    config: &PairConfig,
    market: &MarketView,
    log: &Logger,
) -> Result<()> {
    // 1. Reference price. Anchored at the current slot so the relative
    //    `expiry_offset` is measured from now.
    let price = Price::from_value(config.reference_price)
        .with_context(|| format!("encode reference price {}", config.reference_price))?;
    let slot = client.get_slot().context("current slot")?;
    log.log(format!(
        "set_reference_price {} (slot {slot})",
        config.reference_price
    ));
    let ix = set_reference_price_ix(leader.pubkey(), market.address, VAULT_IDX, price, slot);
    chain::send(client, wallet, &[wallet, leader], &[ix]).context("set_reference_price")?;

    // 2. Quote ladder — a symmetric one-level profile.
    log.log("set_liquidity_profile");
    let bytes = symmetric_profile_bytes(config.offset_ppm, config.size_bps, config.expiry_offset);
    let ix = set_liquidity_profile_ix(leader.pubkey(), market.address, VAULT_IDX, bytes);
    chain::send(client, wallet, &[wallet, leader], &[ix]).context("set_liquidity_profile")?;

    // 3. Fund the leader's ATAs (admin is the mint authority), then seed.
    let (base_atoms, quote_atoms) = config.leader_deposit;
    let base_ata =
        chain::create_ata_idempotent(client, wallet, &leader.pubkey(), &market.base_mint)
            .context("leader base ATA")?;
    let quote_ata =
        chain::create_ata_idempotent(client, wallet, &leader.pubkey(), &market.quote_mint)
            .context("leader quote ATA")?;
    chain::mint_to(client, wallet, &market.base_mint, &base_ata, base_atoms)
        .context("mint base to leader")?;
    chain::mint_to(client, wallet, &market.quote_mint, &quote_ata, quote_atoms)
        .context("mint quote to leader")?;
    log.log(format!(
        "deposit_leader {} {} / {} {}",
        base_atoms, config.base.symbol, quote_atoms, config.quote.symbol
    ));
    let ix = chain::build_deposit_leader_ix(
        &leader.pubkey(),
        &market.address,
        &market.base_mint,
        &market.quote_mint,
        &market.base_treasury,
        &market.quote_treasury,
        VAULT_IDX,
        base_atoms,
        quote_atoms,
    );
    chain::send(client, wallet, &[wallet, leader], &[ix]).context("deposit_leader")?;
    Ok(())
}

/// Load a checked-in keypair named relative to the repo root.
fn load_key(repo_root: &Path, rel: &str) -> Result<Keypair> {
    let path = repo_root.join(rel);
    solana_keypair::read_keypair_file(&path)
        .map_err(|e| anyhow::anyhow!("read keypair {}: {e}", path.display()))
}

/// Serialize a symmetric one-level [`LiquidityProfile`] — the same
/// `±offset_ppm` / `size_bps` / `expiry_offset` on the top bid and ask — to
/// the 160-byte `set_liquidity_profile` argument.
fn symmetric_profile_bytes(offset_ppm: u32, size_bps: u16, expiry_offset: u32) -> [u8; 160] {
    let mut profile = LiquidityProfile::zeroed();
    for side in [&mut profile.bids, &mut profile.asks] {
        side[0].price_offset = offset_ppm.into();
        side[0].size_bps = size_bps.into();
        side[0].expiry_offset = expiry_offset.into();
    }
    profile_bytes(&profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default pair's reference price must encode, and the leader's
    /// opening inventory must be balanced at it (quote = base × price), so
    /// the seeded vault is symmetric around its own quote.
    #[test]
    fn mock_pair_inventory_is_balanced_at_the_reference() {
        let c = &MOCK_CADC_USDC;
        assert!(Price::from_value(c.reference_price).is_some());
        let (base, quote) = c.leader_deposit;
        let expected_quote = (base as f64 * c.reference_price) as u64;
        assert_eq!(quote, expected_quote);
    }

    /// The leader must differ from the admin so `create_vault` doesn't trip
    /// anchor-v2's duplicate-mutable-account rule — they are distinct
    /// checked-in role keys, so the file names at least must differ.
    #[test]
    fn leader_key_is_not_a_pair_mint() {
        let c = &MOCK_CADC_USDC;
        assert_ne!(c.leader_keypair_file, c.base.keypair_file);
        assert_ne!(c.leader_keypair_file, c.quote.keypair_file);
    }

    /// A symmetric profile fills exactly the top bid and ask, leaving the
    /// rest of the ladder zeroed.
    #[test]
    fn symmetric_profile_fills_top_of_book_only() {
        let bytes = symmetric_profile_bytes(5_000, 10_000, u32::MAX);
        assert_eq!(bytes.len(), 160);
        let profile: &LiquidityProfile = bytemuck::from_bytes(&bytes);
        assert_eq!(profile.bids[0].price_offset.get(), 5_000);
        assert_eq!(profile.bids[0].size_bps.get(), 10_000);
        assert_eq!(profile.asks[0].price_offset.get(), 5_000);
        assert_eq!(profile.asks[0].size_bps.get(), 10_000);
        assert_eq!(profile.bids[1].size_bps.get(), 0);
        assert_eq!(profile.asks[1].size_bps.get(), 0);
    }
}
