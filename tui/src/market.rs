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

/// The USD value each side of a seeded vault opens with — the demo's $100
/// top-of-book per market. The leader deposits ≈ `$100` of the base token and
/// `$100` of USDC, balanced at the seed reference, so the opening book is
/// symmetric and the maker bot's full-leg ladder quotes ≈ $100 a side.
pub const SEED_USD_PER_SIDE: f64 = 100.0;

/// A localnet market pair plus everything the bootstrap needs to bring it
/// up quotable and seeded: the two mints, the leader role key that quotes
/// and seeds the vault, the seed reference price, and a symmetric quote
/// ladder. The opening deposit is derived from the price + decimals so each
/// side opens at [`SEED_USD_PER_SIDE`]. One per tradeable pair.
pub struct PairConfig {
    pub base: MintSpec,
    pub quote: MintSpec,
    /// Leader + quote-authority keypair (a checked-in role key). Must not be
    /// the admin wallet — anchor-v2 rejects the same key in the admin and
    /// leader slots of `create_vault`.
    pub leader_keypair_file: &'static str,
    /// Seed (quote-per-base) price in human units, e.g. `1.14` USDC per EURC.
    /// Just the opening anchor — the maker bot rediscovers the live price from
    /// its feeds and re-stamps it. Tokens span orders of magnitude (EURC
    /// ~$1.14 … IDRX ~$0.000056), so this is converted to the on-chain
    /// atoms-ratio per [`reference_atoms_ratio`] before encoding.
    pub reference_price: f64,
    /// How many slots after the quote each ladder rung expires. The bootstrap
    /// stamps this on the seeded profile; `u32::MAX` means never (the maker
    /// re-arms expiry itself once it takes over). The rung geometry (offsets and
    /// per-side depth) lives in [`SEED_LADDER`] / [`ladder_at_spread_bps`], not
    /// here — every market opens with the same shape.
    pub expiry_offset: u32,
}

/// The seven FX-stablecoin markets the localnet bootstrap brings up, each a
/// `<token>/USDC` pair led by the `EEEE` role key with a full-inventory ±0.5%
/// opening ladder. The base mints are the mock localnet keypairs in `keys/`
/// (vanity-named for the token); decimals match the real tokens so the
/// localnet plumbing exercises the same per-market decimal handling the
/// devnet/mainnet promotion will. Listing a `PairConfig` here also teaches the
/// accounts pane its mint tickers (see [`mint_symbols`]).
const fn fx_market(
    symbol: &'static str,
    keypair_file: &'static str,
    decimals: u8,
    reference_price: f64,
) -> PairConfig {
    PairConfig {
        base: MintSpec {
            symbol,
            keypair_file,
            decimals,
        },
        quote: MintSpec {
            symbol: "USDC",
            keypair_file: "keys/USDC.json",
            decimals: 6,
        },
        leader_keypair_file: "keys/EEEE.json",
        reference_price,
        expiry_offset: u32::MAX, // never expires (re-armed by the maker bot)
    }
}

pub const MARKET_EURC: PairConfig = fx_market("EURC", "keys/EURC.json", 6, 1.14);
pub const MARKET_VCHF: PairConfig = fx_market("VCHF", "keys/VCHF.json", 9, 1.235);
pub const MARKET_TGBP: PairConfig = fx_market("TGBP", "keys/TGBP.json", 9, 1.324);
pub const MARKET_ZARP: PairConfig = fx_market("ZARP", "keys/ZARP.json", 6, 0.0605);
pub const MARKET_MXNE: PairConfig = fx_market("MXNe", "keys/MXNe.json", 9, 0.0573);
pub const MARKET_XSGD: PairConfig = fx_market("XSGD", "keys/XSGD.json", 6, 0.7705);
pub const MARKET_IDRX: PairConfig = fx_market("IDRX", "keys/idrx.json", 2, 0.000056);

/// Every localnet pair the bootstrap can bring up.
pub const PAIRS: [&PairConfig; 7] = [
    &MARKET_EURC,
    &MARKET_VCHF,
    &MARKET_TGBP,
    &MARKET_ZARP,
    &MARKET_MXNE,
    &MARKET_XSGD,
    &MARKET_IDRX,
];

/// The opening / reset quote ladder: a four-rung symmetric ladder of
/// `(offset_ppm, size_bps)` mirroring the maker bot's own `DEFAULT_LADDER`
/// (`bots/maker-bot`). The bootstrap seeds the book with this so it opens with
/// visible depth across several price levels — not the single rung the maker
/// only later fans out — and the TUI's "Reset ladder" reshape returns to it.
/// Offsets are relative ppm and sizes are bps of the inventory leg (Σ = 10000,
/// the full per-side commit), so the ladder is market-agnostic. Widths spread
/// ±0.5% / ±1% / ±2% / ±5% with depth thinning outward.
pub const SEED_LADDER: [(u32, u16); 4] = [
    (5_000, 4_000),
    (10_000, 3_000),
    (20_000, 2_000),
    (50_000, 1_000),
];

/// The default top-of-book bid-ask spread (bps) the book opens at and the eCLOB
/// widen / tighten controls step from. [`SEED_LADDER`]'s top rung (5000 ppm =
/// ±0.5% = a 100 bps spread) is the shape template; the default scales it to
/// this tighter opening spread.
pub const DEFAULT_SPREAD_BPS: u32 = 50;

/// The seed ladder scaled to a target bid-ask `spread_bps` — the shape of
/// [`SEED_LADDER`] with its rung offsets scaled so the top rung yields
/// `spread_bps` at the top of book (the seed's 5000 ppm top ≡ 100 bps, so the
/// scale is `spread_bps / 100`). The book opens at [`DEFAULT_SPREAD_BPS`] and
/// the widen / tighten controls step this by ±5 bps, keeping all four levels.
pub fn ladder_at_spread_bps(spread_bps: u32) -> [(u32, u16); 4] {
    seed_ladder_scaled_offsets(spread_bps as f64 / 100.0)
}

/// Convert a pair's human quote-per-base price into the atoms-ratio the
/// on-chain `Price` encodes — `quote_atoms` per `base_atoms`. They coincide
/// only when both legs share decimals; a token with more decimals than USDC
/// scales the ratio down, fewer scales it up.
pub fn reference_atoms_ratio(config: &PairConfig) -> f64 {
    config.reference_price * 10f64.powi(config.quote.decimals as i32 - config.base.decimals as i32)
}

/// The leader's opening deposit `(base_atoms, quote_atoms)`, sized so each leg
/// is worth [`SEED_USD_PER_SIDE`] at the seed reference and the vault opens
/// balanced. USDC (the quote) is ≈ $1, so its side is just the dollar amount;
/// the base side is the token quantity worth the same, scaled by decimals.
pub fn seed_deposit(config: &PairConfig) -> (u64, u64) {
    let quote_atoms = (SEED_USD_PER_SIDE * 10f64.powi(config.quote.decimals as i32)) as u64;
    let base_units = SEED_USD_PER_SIDE / config.reference_price;
    let base_atoms = (base_units * 10f64.powi(config.base.decimals as i32)) as u64;
    (base_atoms, quote_atoms)
}

/// Resolve each known pair's mint address → human ticker, loading the
/// checked-in mint keypairs once. The chain scan that discovers a market only
/// yields mint pubkeys, so the accounts pane needs this to label a market with
/// its coins. A pair whose keypair files don't load is skipped — its mints
/// fall back to the generic base/quote labels.
pub fn mint_symbols(repo_root: &Path) -> Vec<(Pubkey, &'static str)> {
    let mut out = Vec::new();
    for pair in PAIRS {
        for spec in [&pair.base, &pair.quote] {
            if let Ok(kp) = load_key(repo_root, spec.keypair_file) {
                out.push((kp.pubkey(), spec.symbol));
            }
        }
    }
    out
}

/// The bootstrap always opens the market's first vault, so its sector index
/// is 0. The seed instructions address the vault by this index.
const VAULT_IDX: u32 = 0;

/// The taker / swapper role key (`keys/README.md`'s `FFFF`). The swap probe
/// signs and pays for its take with this, so the swapper is a distinct,
/// recognizable participant — never the admin. Not pair-specific: one taker
/// exercises any market.
const TAKER_KEYPAIR_FILE: &str = "keys/FFFF.json";

/// Load the leader / quote-authority keypair `config` names.
pub fn leader(repo_root: &Path, config: &PairConfig) -> Result<Keypair> {
    load_key(repo_root, config.leader_keypair_file)
}

/// The `PairConfig` whose base mint is `base_mint`, resolving it by loading the
/// checked-in mint keypairs and matching addresses — `None` for a market minted
/// outside the bootstrap roster. Lets a market-scoped control (the eCLOB
/// reprice / reshape keybinds) recover the selected market's seed ladder and
/// leader key from just its base mint.
pub fn config_for(repo_root: &Path, base_mint: &Pubkey) -> Option<&'static PairConfig> {
    PAIRS.into_iter().find(|c| {
        load_key(repo_root, c.base.keypair_file)
            .map(|k| k.pubkey() == *base_mint)
            .unwrap_or(false)
    })
}

/// Load the leader / quote-authority keypair for the market with `base_mint` —
/// the signer `set_reference_price` / `set_liquidity_profile` require.
pub fn leader_for(repo_root: &Path, base_mint: &Pubkey) -> Result<Keypair> {
    let config = config_for(repo_root, base_mint).context("market not in the bootstrap roster")?;
    leader(repo_root, config)
}

/// Resolve a pair's two mint pubkeys from its checked-in keypair files without
/// creating them — used to address an already-created market (its PDA is
/// seeded on `[base, quote]`).
pub fn pair_mints(repo_root: &Path, config: &PairConfig) -> Result<(Pubkey, Pubkey)> {
    let base = load_key(repo_root, config.base.keypair_file)?;
    let quote = load_key(repo_root, config.quote.keypair_file)?;
    Ok((base.pubkey(), quote.pubkey()))
}

/// Load the taker / swapper role key (`FFFF`) — the probe swap's signer.
pub fn taker(repo_root: &Path) -> Result<Keypair> {
    load_key(repo_root, TAKER_KEYPAIR_FILE)
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
    // 1. Reference price. The engine stores the atoms-ratio, so scale the
    //    human price by the decimal gap before encoding. Anchored at the
    //    current slot so the relative `expiry_offset` is measured from now.
    let ratio = reference_atoms_ratio(config);
    let price = Price::from_value(ratio).with_context(|| {
        format!(
            "encode reference price {} (atoms-ratio {ratio})",
            config.reference_price
        )
    })?;
    let slot = client.get_slot().context("current slot")?;
    log.log(format!(
        "set_reference_price {} (slot {slot})",
        config.reference_price
    ));
    let ix = set_reference_price_ix(leader.pubkey(), market.address, VAULT_IDX, price, slot);
    chain::send_logged(
        client,
        wallet,
        &[wallet, leader],
        &[ix],
        "set_reference_price",
        log,
    )
    .context("set_reference_price")?;

    // 2. Quote ladder — a multi-rung symmetric ladder at the default spread, so
    //    the book opens with depth across several price levels (not a single
    //    rung the maker only later fans out) and at the tighter default spread.
    log.log("set_liquidity_profile");
    let bytes = ladder_profile_bytes(
        &ladder_at_spread_bps(DEFAULT_SPREAD_BPS),
        config.expiry_offset,
    );
    let ix = set_liquidity_profile_ix(leader.pubkey(), market.address, VAULT_IDX, bytes);
    chain::send_logged(
        client,
        wallet,
        &[wallet, leader],
        &[ix],
        "set_liquidity_profile",
        log,
    )
    .context("set_liquidity_profile")?;

    // 3. Fund the leader's ATAs (admin is the mint authority), then seed.
    let (base_atoms, quote_atoms) = seed_deposit(config);
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
    chain::send_logged(
        client,
        wallet,
        &[wallet, leader],
        &[ix],
        "deposit_leader",
        log,
    )
    .context("deposit_leader")?;
    Ok(())
}

/// Load a checked-in keypair named relative to the repo root.
fn load_key(repo_root: &Path, rel: &str) -> Result<Keypair> {
    let path = repo_root.join(rel);
    solana_keypair::read_keypair_file(&path)
        .map_err(|e| anyhow::anyhow!("read keypair {}: {e}", path.display()))
}

/// Serialize an asymmetric multi-rung ladder — an independent
/// `(offset_ppm, size_bps)` list per side, sharing `expiry_offset` — to the
/// 160-byte `set_liquidity_profile` argument. Rungs past the profile's capacity
/// are dropped. The symmetric [`ladder_profile_bytes`] is the common case; the
/// "thin the far side" reshape uses the asymmetric form (a full bid ladder over
/// a thinned ask ladder), so the book's shape is a direct readout of both
/// sides' rungs.
pub fn ladder_profile_bytes_asym(
    bids: &[(u32, u16)],
    asks: &[(u32, u16)],
    expiry_offset: u32,
) -> [u8; 160] {
    let mut profile = LiquidityProfile::zeroed();
    for (i, &(offset_ppm, size_bps)) in bids.iter().take(profile.bids.len()).enumerate() {
        profile.bids[i].price_offset = offset_ppm.into();
        profile.bids[i].size_bps = size_bps.into();
        profile.bids[i].expiry_offset = expiry_offset.into();
    }
    for (i, &(offset_ppm, size_bps)) in asks.iter().take(profile.asks.len()).enumerate() {
        profile.asks[i].price_offset = offset_ppm.into();
        profile.asks[i].size_bps = size_bps.into();
        profile.asks[i].expiry_offset = expiry_offset.into();
    }
    profile_bytes(&profile)
}

/// Serialize a symmetric multi-rung ladder — the same `(offset_ppm, size_bps)`
/// list on both sides — to the 160-byte `set_liquidity_profile` argument. The
/// bootstrap seeds the opening book with [`SEED_LADDER`] through here (so it
/// opens with several price levels of depth), and the TUI's widen / tighten /
/// reset reshapes all encode a full multi-level ladder through here too.
pub fn ladder_profile_bytes(rungs: &[(u32, u16)], expiry_offset: u32) -> [u8; 160] {
    ladder_profile_bytes_asym(rungs, rungs, expiry_offset)
}

/// The seed ladder with every rung's price offset scaled by `scale` (sizes
/// unchanged) — a widen (`scale > 1`) or tighten (`scale < 1`) reshape that
/// keeps all four rungs, so the book stays multi-level and only the spread
/// moves.
pub fn seed_ladder_scaled_offsets(scale: f64) -> [(u32, u16); 4] {
    let mut out = SEED_LADDER;
    for (offset_ppm, _) in &mut out {
        *offset_ppm = ((*offset_ppm as f64) * scale).round() as u32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every market's reference price must encode as a `Price` once scaled to
    /// the atoms-ratio — the wide unit-price spread (EURC ~$1.14 down to IDRX
    /// ~$0.000056) plus mixed decimals all has to land inside the codec range.
    #[test]
    fn every_market_reference_encodes() {
        for c in PAIRS {
            let ratio = reference_atoms_ratio(c);
            assert!(
                Price::from_value(ratio).is_some(),
                "{} ratio {ratio} out of Price range",
                c.base.symbol
            );
        }
    }

    /// Each market's seed deposit opens both legs at ≈ $100, so the vault is
    /// symmetric around its own quote regardless of the token's decimals.
    #[test]
    fn seed_deposit_is_balanced_at_one_hundred_usd_per_side() {
        for c in PAIRS {
            let (base_atoms, quote_atoms) = seed_deposit(c);
            let base_usd =
                base_atoms as f64 / 10f64.powi(c.base.decimals as i32) * c.reference_price;
            let quote_usd = quote_atoms as f64 / 10f64.powi(c.quote.decimals as i32);
            assert!(
                (base_usd - SEED_USD_PER_SIDE).abs() < 1.0,
                "{} base side ${base_usd}",
                c.base.symbol
            );
            assert!(
                (quote_usd - SEED_USD_PER_SIDE).abs() < 0.01,
                "{} quote side ${quote_usd}",
                c.base.symbol
            );
        }
    }

    /// The leader must differ from each pair's mints so `create_vault` doesn't
    /// trip anchor-v2's duplicate-mutable-account rule — they are distinct
    /// checked-in role keys, so the file names at least must differ. The quote
    /// is the shared USDC mint across every market.
    #[test]
    fn leader_key_is_not_a_pair_mint() {
        for c in PAIRS {
            assert_ne!(c.leader_keypair_file, c.base.keypair_file);
            assert_ne!(c.leader_keypair_file, c.quote.keypair_file);
            assert_eq!(c.quote.keypair_file, "keys/USDC.json");
        }
    }

    /// An asymmetric ladder encodes each side's own rungs — the shape behind
    /// "thin the far side": a full bid ladder over a thinned ask ladder.
    #[test]
    fn asymmetric_ladder_encodes_each_side_independently() {
        // Thinned asks: the seed ladder with each rung's depth scaled to 30% —
        // built inline the way `do_reshape`'s thin-far-side path does.
        let mut asks = SEED_LADDER;
        for (_, size_bps) in &mut asks {
            *size_bps = (*size_bps as f64 * 0.3).round() as u16;
        }
        let bytes = ladder_profile_bytes_asym(&SEED_LADDER, &asks, u32::MAX);
        let profile: &LiquidityProfile = bytemuck::from_bytes(&bytes);
        // Bid stays at the full seed ladder; ask depth is thinned to 30%.
        assert_eq!(profile.bids[0].size_bps.get(), SEED_LADDER[0].1);
        assert_eq!(
            profile.asks[0].size_bps.get(),
            (SEED_LADDER[0].1 as f64 * 0.3).round() as u16
        );
        // Offsets are untouched on both sides.
        assert_eq!(profile.bids[0].price_offset.get(), SEED_LADDER[0].0);
        assert_eq!(profile.asks[0].price_offset.get(), SEED_LADDER[0].0);
    }

    /// The spread→ladder mapping: the default 50 bps halves the seed's 100-bps
    /// top rung, and 100 bps reproduces the seed offsets exactly — every rung
    /// stays, depths unchanged.
    #[test]
    fn ladder_at_spread_scales_offsets_to_the_target() {
        let half = ladder_at_spread_bps(DEFAULT_SPREAD_BPS);
        let full = ladder_at_spread_bps(100);
        for (i, seed) in SEED_LADDER.iter().enumerate() {
            assert_eq!(half[i].0, seed.0 / 2);
            assert_eq!(half[i].1, seed.1);
            assert_eq!(full[i].0, seed.0);
        }
    }

    /// Scaling the seed ladder's offsets fans (or pulls) every rung by the same
    /// factor while keeping all four rungs and their depths — the widen /
    /// tighten reshape that stays multi-level.
    #[test]
    fn scaled_offsets_move_every_rung_and_keep_depth() {
        let wide = seed_ladder_scaled_offsets(3.0);
        assert_eq!(wide.len(), SEED_LADDER.len());
        for (scaled, seed) in wide.iter().zip(SEED_LADDER.iter()) {
            assert_eq!(scaled.0, seed.0 * 3);
            assert_eq!(scaled.1, seed.1);
        }
    }

    /// The seed ladder serializes symmetrically across every rung, leaving the
    /// levels past its length zeroed — the multi-level book the bootstrap opens.
    #[test]
    fn seed_ladder_fills_every_rung_symmetrically() {
        let bytes = ladder_profile_bytes(&SEED_LADDER, u32::MAX);
        assert_eq!(bytes.len(), 160);
        let profile: &LiquidityProfile = bytemuck::from_bytes(&bytes);
        for (i, &(offset_ppm, size_bps)) in SEED_LADDER.iter().enumerate() {
            assert_eq!(profile.bids[i].price_offset.get(), offset_ppm);
            assert_eq!(profile.bids[i].size_bps.get(), size_bps);
            assert_eq!(profile.asks[i].price_offset.get(), offset_ppm);
            assert_eq!(profile.asks[i].size_bps.get(), size_bps);
        }
        // The rung past the ladder's length stays zeroed.
        assert_eq!(profile.bids[SEED_LADDER.len()].size_bps.get(), 0);
        assert_eq!(profile.asks[SEED_LADDER.len()].size_bps.get(), 0);
    }

    /// The seed ladder commits the full inventory leg per side (Σ = 10000 bps),
    /// matching the maker's own ladder invariant, and its widths thin outward.
    #[test]
    fn seed_ladder_fully_commits_each_side_and_thins_outward() {
        let total: u32 = SEED_LADDER.iter().map(|(_, bps)| *bps as u32).sum();
        assert_eq!(total, 10_000);
        for w in SEED_LADDER.windows(2) {
            assert!(w[1].0 > w[0].0, "offsets must widen outward");
            assert!(w[1].1 < w[0].1, "depth must thin outward");
        }
    }
}
