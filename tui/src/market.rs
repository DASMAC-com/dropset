//! Reusable, config-driven market bootstrap.
//!
//! The TUI's market setup used to mint a throwaway random base/quote pair
//! on every run, so each bootstrap produced a *different* market address —
//! fine for driving the TUI alone, useless for anything that needs a stable
//! target (a market-making bot, the explorer). This module generalizes it:
//! a [`PairConfig`] names two **fixed, checked-in** mint keypairs (so the
//! market PDA, seeded on `[base, quote]`, is the same address every run), a
//! leader role key, and the quote ladder / seed deposit that bring the vault
//! up shaped and funded. Drop in another pair by adding another `PairConfig`;
//! nothing else changes.
//!
//! Markets deliberately open **dark**: no reference price is stamped, so
//! nothing matches until a maker bot quotes the market. [`seed_vault`] has
//! the reasoning.
//!
//! The localnet keys live under `keys/` (see `keys/README.md`); their paths
//! resolve against the repo root the TUI already locates. The admin wallet
//! is the fee payer and mint authority for every transaction here; the
//! leader co-signs only the vault-gated instructions (`set_liquidity_profile`
//! and `deposit_leader` here, `set_reference_price` once a maker bot or the
//! eCLOB controls quote), so it needs no SOL balance.
//!
//! Mainnet is a second roster, [`MAINNET_PAIRS`], over the **same**
//! [`PairConfig`] type rather than a parallel one. What differs is where each
//! address comes from: a localnet mint is a [`MintKey::Keypair`] the bootstrap
//! creates, a mainnet mint is a [`MintKey::Existing`] address that is only ever
//! verified, never created — minting one would issue counterfeit tokens — and a
//! mainnet leader is [`LeaderKey::Operator`], arriving at launch rather than
//! from a committed file. [`roster`] picks between them by cluster.

// cspell:word keypairs

use crate::accounts::MarketView;
use crate::chain;
use crate::cluster::Cluster;
use crate::job::Logger;
use anyhow::{Context, Result};
use bytemuck::Zeroable;
use dropset_sdk::clock::{SlotSpan, WallSpan};
use dropset_sdk::layout::LiquidityProfile;
use dropset_sdk::quoting::{profile_bytes, set_liquidity_profile_ix, PROFILE_BYTES};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use std::path::Path;

/// Where a mint's address comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MintKey {
    /// A checked-in localnet keypair, named relative to the repo root. The
    /// bootstrap creates the mint at this keypair's address.
    Keypair(&'static str),
    /// A real mint that already exists on its cluster. Verified before use and
    /// never created: the address is the issuer's, not ours.
    Existing(Pubkey),
}

/// One SPL mint in a pair: where its address comes from, a human symbol for
/// the log, and its decimals.
pub struct MintSpec {
    pub symbol: &'static str,
    pub key: MintKey,
    pub decimals: u8,
}

impl MintSpec {
    /// This mint's address — loaded from its keypair file, or the fixed
    /// address itself.
    pub fn address(&self, repo_root: &Path) -> Result<Pubkey> {
        match self.key {
            MintKey::Keypair(file) => Ok(load_key(repo_root, file)?.pubkey()),
            MintKey::Existing(address) => Ok(address),
        }
    }
}

/// Where a pair's leader / quote-authority key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaderKey {
    /// A checked-in localnet role key, named relative to the repo root.
    Keypair(&'static str),
    /// Supplied by the operator at launch (`--leader`). The committed role
    /// keys lead nothing real, so a mainnet pair names no file at all.
    Operator,
}

/// The USD value each side of a seeded vault opens with — the demo's $100
/// top-of-book per market. The leader deposits ≈ `$100` of the base token and
/// `$100` of USDC, balanced at the seed reference, so the opening book is
/// symmetric and the maker bot's full-leg ladder quotes ≈ $100 a side.
pub const SEED_USD_PER_SIDE: f64 = 100.0;

/// A localnet market pair plus everything the bootstrap needs to bring it
/// up shaped and seeded: the two mints, the leader role key that quotes
/// and seeds the vault, the reference price the deposit is balanced at, and
/// a symmetric quote ladder. The opening deposit is derived from the price +
/// decimals so each side opens at [`SEED_USD_PER_SIDE`]. One per tradeable
/// pair.
pub struct PairConfig {
    pub base: MintSpec,
    pub quote: MintSpec,
    /// Leader + quote-authority key. Must not be the admin wallet — anchor-v2
    /// rejects the same key in the admin and leader slots of `create_vault`.
    pub leader: LeaderKey,
    /// Expected (quote-per-base) price in human units, e.g. `1.14` USDC per
    /// EURC. Nothing stamps it on chain — the maker bot discovers the live
    /// price from its feeds and stamps that — so it serves only to balance the
    /// opening deposit at [`SEED_USD_PER_SIDE`] a side (see [`seed_deposit`]),
    /// and to check the pair is quotable at all: tokens span orders of
    /// magnitude (EURC ~$1.14 … IDRX ~$0.000056), and the price the maker will
    /// stamp has to survive the conversion to the on-chain atoms-ratio (see
    /// [`dropset_util::decimals::human_to_atoms_ratio`], which this module's
    /// tests range-check every pair through).
    pub reference_price: f64,
    /// How long after the quote each ladder rung expires, in the **wall**
    /// domain. The bootstrap stamps this on the seeded profile;
    /// [`WallSpan::UNBOUNDED`] means never (the maker re-arms expiry
    /// itself once it takes over). The seeded book is left
    /// slot-unbounded too — the maker's own ladder is what introduces
    /// per-tier slot bounds. The rung geometry (offsets and per-side
    /// depth) lives in [`SEED_LADDER`] / [`ladder_at_spread_bps`], not
    /// here — every market opens with the same shape.
    pub expiry_offset_secs: WallSpan,
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
            key: MintKey::Keypair(keypair_file),
            decimals,
        },
        quote: MintSpec {
            symbol: "USDC",
            key: MintKey::Keypair("keys/USDC.json"),
            decimals: 6,
        },
        leader: LeaderKey::Keypair("keys/EEEE.json"),
        reference_price,
        // Never expires in wall time; re-armed by the maker bot.
        expiry_offset_secs: WallSpan::UNBOUNDED,
    }
}

/// Circle's mainnet USDC — the quote leg of every mainnet pair, and the mint
/// the mainnet registry charges its per-vault fee in.
pub const MAINNET_USDC: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

/// A mainnet `<token>/USDC` pair over a real, existing base mint, led by the
/// operator-supplied leader. Shares [`fx_market`]'s ladder, expiry and seed
/// reference so a mainnet vault opens with the same shape and sizing as its
/// localnet rehearsal.
const fn mainnet_fx_market(
    symbol: &'static str,
    mint: Pubkey,
    decimals: u8,
    reference_price: f64,
) -> PairConfig {
    PairConfig {
        base: MintSpec {
            symbol,
            key: MintKey::Existing(mint),
            decimals,
        },
        quote: MintSpec {
            symbol: "USDC",
            key: MintKey::Existing(MAINNET_USDC),
            decimals: 6,
        },
        leader: LeaderKey::Operator,
        reference_price,
        expiry_offset_secs: WallSpan::UNBOUNDED,
    }
}

pub const MARKET_EURC: PairConfig = fx_market("EURC", "keys/EURC.json", 6, 1.14);
pub const MARKET_VCHF: PairConfig = fx_market("VCHF", "keys/VCHF.json", 9, 1.235);
pub const MARKET_TGBP: PairConfig = fx_market("TGBP", "keys/TGBP.json", 9, 1.324);
pub const MARKET_ZARP: PairConfig = fx_market("ZARP", "keys/ZARP.json", 6, 0.0605);
pub const MARKET_MXNE: PairConfig = fx_market("MXNe", "keys/MXNe.json", 9, 0.0573);
pub const MARKET_XSGD: PairConfig = fx_market("XSGD", "keys/XSGD.json", 6, 0.7705);
pub const MARKET_IDRX: PairConfig = fx_market("IDRX", "keys/idrx.json", 2, 0.000056);
// The two MVP pairs. Their seed references are the ECB fix for 2026-09-08
// (USD/AUD 1.3861 and USD/CAD 1.3805, reciprocated), the same
// representative-spot basis the rest of the roster uses — the maker discovers
// the live price from the feeds and never reads these after bootstrap.
pub const MARKET_AUDD: PairConfig = fx_market("AUDD", "keys/AUDD.json", 6, 0.7214);
pub const MARKET_CADC: PairConfig = fx_market("CADC", "keys/CADC.json", 6, 0.7244);

/// Every localnet pair the bootstrap can bring up.
pub const PAIRS: [&PairConfig; 9] = [
    &MARKET_EURC,
    &MARKET_VCHF,
    &MARKET_TGBP,
    &MARKET_ZARP,
    &MARKET_MXNE,
    &MARKET_XSGD,
    &MARKET_IDRX,
    &MARKET_AUDD,
    &MARKET_CADC,
];

// The mainnet MVP pairs. Addresses and decimals are the issuers' own, as
// recorded in the frontend's `currencies.json` — the test
// `mainnet_mints_match_the_frontend_currency_data` holds the two copies equal,
// so neither can drift from the other unnoticed.
pub const MAINNET_EURC: PairConfig = mainnet_fx_market(
    "EURC",
    pubkey!("HzwqbKZw8HxMN6bF2yFZNrht3c2iXXzpKcFu7uBEDKtr"),
    6,
    1.14,
);
pub const MAINNET_AUDD: PairConfig = mainnet_fx_market(
    "AUDD",
    pubkey!("AUDDttiEpCydTm7joUMbYddm72jAWXZnCpPZtDoxqBSw"),
    6,
    0.7214,
);
pub const MAINNET_CADC: PairConfig = mainnet_fx_market(
    "CADC",
    pubkey!("9ewjJpmD1ES83RDRFnHs7V2hUdH76WAVjdvu6UV6WNo7"),
    6,
    0.7244,
);

/// Every mainnet pair the ceremony brings up.
pub const MAINNET_PAIRS: [&PairConfig; 3] = [&MAINNET_EURC, &MAINNET_AUDD, &MAINNET_CADC];

/// The pairs a `cluster` session brings up.
pub fn roster(cluster: Cluster) -> &'static [&'static PairConfig] {
    match cluster {
        Cluster::Localnet => &PAIRS,
        Cluster::Mainnet => &MAINNET_PAIRS,
    }
}

/// The market PDA of every pair in `cluster`'s roster — what the phase gate
/// measures progress against, so a ceremony that stopped part-way still reads
/// as unfinished. A pair whose mints do not resolve is skipped.
pub fn roster_markets(repo_root: &Path, cluster: Cluster) -> Vec<Pubkey> {
    roster(cluster)
        .iter()
        .filter_map(|c| pair_mints(repo_root, c).ok())
        .map(|(base, quote)| chain::market_pda(&base, &quote))
        .collect()
}

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

/// Resolve each `cluster` pair's mint address → human ticker. The chain scan
/// that discovers a market only yields mint pubkeys, so the accounts pane needs
/// this to label a market with its coins. A mint whose address doesn't resolve
/// (a localnet keypair file that won't load) is skipped — it falls back to the
/// generic base/quote labels.
pub fn mint_symbols(repo_root: &Path, cluster: Cluster) -> Vec<(Pubkey, &'static str)> {
    let mut out = Vec::new();
    for pair in roster(cluster) {
        for spec in [&pair.base, &pair.quote] {
            if let Ok(address) = spec.address(repo_root) {
                out.push((address, spec.symbol));
            }
        }
    }
    out
}

/// The taker / swapper role key (`keys/README.md`'s `FFFF`). The swap probe
/// signs and pays for its take with this, so the swapper is a distinct,
/// recognizable participant — never the admin. Not pair-specific: one taker
/// exercises any market.
const TAKER_KEYPAIR_FILE: &str = "keys/FFFF.json";

/// Load the leader / quote-authority keypair `config` names.
///
/// An [`LeaderKey::Operator`] pair names no file, so this needs the key the
/// operator supplied at launch as `operator`; without it, it errors rather
/// than falling back to a committed role key that leads nothing real.
pub fn leader(
    repo_root: &Path,
    config: &PairConfig,
    operator: Option<&Keypair>,
) -> Result<Keypair> {
    match config.leader {
        LeaderKey::Keypair(file) => load_key(repo_root, file),
        LeaderKey::Operator => operator.map(Keypair::insecure_clone).with_context(|| {
            format!(
                "the {} leader is operator-supplied on this cluster — relaunch with \
                 --leader <keypair>",
                config.base.symbol
            )
        }),
    }
}

/// The `cluster` pair whose base mint is `base_mint` — `None` for a market
/// outside the roster. Lets a market-scoped control (the eCLOB reprice /
/// reshape keybinds) recover the selected market's seed ladder and leader key
/// from just its base mint.
pub fn config_for(
    repo_root: &Path,
    cluster: Cluster,
    base_mint: &Pubkey,
) -> Option<&'static PairConfig> {
    roster(cluster)
        .iter()
        .copied()
        .find(|c| c.base.address(repo_root).is_ok_and(|a| a == *base_mint))
}

/// Load the localnet leader / quote-authority keypair for the market with
/// `base_mint` — the signer `set_reference_price` / `set_liquidity_profile`
/// require. Localnet only: the eCLOB controls that call it are unavailable on
/// mainnet.
pub fn leader_for(repo_root: &Path, base_mint: &Pubkey) -> Result<Keypair> {
    let config = config_for(repo_root, Cluster::Localnet, base_mint)
        .context("market not in the bootstrap roster")?;
    leader(repo_root, config, None)
}

/// Resolve a pair's two mint pubkeys without creating them — used to address
/// an already-created market (its PDA is seeded on `[base, quote]`).
pub fn pair_mints(repo_root: &Path, config: &PairConfig) -> Result<(Pubkey, Pubkey)> {
    Ok((
        config.base.address(repo_root)?,
        config.quote.address(repo_root)?,
    ))
}

/// Load the taker / swapper role key (`FFFF`) — the probe swap's signer.
pub fn taker(repo_root: &Path) -> Result<Keypair> {
    load_key(repo_root, TAKER_KEYPAIR_FILE)
}

/// Make `config`'s two mints usable and return `(base_mint, quote_mint)`.
///
/// A [`MintKey::Keypair`] mint is created at its checked-in address with the
/// admin `wallet` as mint authority (existence-checked, so a re-run is a
/// no-op). A [`MintKey::Existing`] mint is **never created** — only verified
/// to exist with the decimals the pair expects, because the address is a real
/// issuer's and anything this crate minted there would be counterfeit.
pub fn ensure_pair_mints(
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    config: &PairConfig,
    log: &Logger,
) -> Result<(Pubkey, Pubkey)> {
    let base = ensure_mint(client, wallet, repo_root, &config.base, log)?;
    let quote = ensure_mint(client, wallet, repo_root, &config.quote, log)?;
    Ok((base, quote))
}

/// [`ensure_pair_mints`]' per-mint half.
fn ensure_mint(
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    spec: &MintSpec,
    log: &Logger,
) -> Result<Pubkey> {
    match spec.key {
        MintKey::Keypair(file) => {
            let kp = load_key(repo_root, file)?;
            log.log(format!("Creating fixed {} mint…", spec.symbol));
            chain::create_mint(client, wallet, &kp, spec.decimals)
                .with_context(|| format!("create {} mint", spec.symbol))?;
            log.log(format!("{}: {}", spec.symbol, kp.pubkey()));
            Ok(kp.pubkey())
        }
        MintKey::Existing(address) => {
            chain::verify_mint(client, &address, spec.decimals)
                .with_context(|| format!("verify the real {} mint", spec.symbol))?;
            log.log(format!(
                "{}: {address} (existing mint, verified)",
                spec.symbol
            ));
            Ok(address)
        }
    }
}

/// Create every localnet pair's shared quote mint once, up front, deduped by
/// address. All FX pairs quote in the same fixed USDC mint, and
/// [`ensure_pair_mints`]' underlying [`chain::create_mint`] is existence-checked but *not*
/// concurrency-safe — two threads creating the same mint would both find it
/// absent and one would fail with "account already in use". So the parallel
/// per-market bootstrap pre-creates the shared quote mint(s) here, sequentially,
/// leaving each market to create only its unique base mint concurrently. The
/// dedup keeps this correct if a future pair ever quotes in something other
/// than USDC.
pub fn ensure_quote_mints(
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    log: &Logger,
) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for config in PAIRS {
        if seen.insert(config.quote.address(repo_root)?) {
            ensure_mint(client, wallet, repo_root, &config.quote, log)
                .with_context(|| format!("create shared quote mint {}", config.quote.symbol))?;
        }
    }
    Ok(())
}

/// Pre-fund each shared leader's quote (USDC) ATA once, up front, with the
/// total quote deposit across the markets that share it — so the parallel
/// bootstrap can skip the per-market quote top-up entirely (see
/// [`seed_vault`]'s `prefunded_quote`).
///
/// Every FX pair deposits from the one leader's one USDC ATA in the identical
/// amount, so if each market minted its own quote top-up concurrently, those
/// `mint_to`s would be byte-identical transactions: workers sharing a blockhash
/// produce the same signature and the validator drops all but one, so most
/// top-ups silently vanish and the deposits then underflow the shared balance
/// (`InsufficientFunds`, 0x1). Front-loading the whole total in one sequential
/// mint here sidesteps that: the pool already covers every deposit, and no
/// colliding per-market quote mint runs at all. The base leg needs no
/// equivalent — each market's base ATA / amount is its own, funded and drawn
/// down within its own thread. Grouped by `(leader, quote mint)` so it stays
/// correct if a future pair uses a different leader or quote.
pub fn prefund_leader_quotes(
    client: &RpcClient,
    wallet: &Keypair,
    repo_root: &Path,
    log: &Logger,
) -> Result<()> {
    let mut totals: std::collections::HashMap<(Pubkey, Pubkey), u64> =
        std::collections::HashMap::new();
    for config in PAIRS {
        let leader = leader(repo_root, config, None)?;
        let (_, quote_mint) = pair_mints(repo_root, config)?;
        let (_, quote_atoms) = seed_deposit(config);
        *totals.entry((leader.pubkey(), quote_mint)).or_default() += quote_atoms;
    }
    for ((leader_pubkey, quote_mint), total) in totals {
        let quote_ata = chain::create_ata_idempotent(client, wallet, &leader_pubkey, &quote_mint)
            .context("leader quote ATA")?;
        log.log(format!(
            "Pre-funding leader {leader_pubkey} quote ATA with {total} atoms…"
        ));
        chain::mint_to(client, wallet, &quote_mint, &quote_ata, total)
            .context("pre-fund leader quote")?;
    }
    Ok(())
}

/// Where [`seed_vault`]'s opening deposit comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Funding {
    /// Localnet: the admin mints both legs to the leader first.
    MintBoth,
    /// Localnet, parallel bootstrap: mint the base leg only. The shared quote
    /// ATA was already filled with the whole deposit total up front (via
    /// [`prefund_leader_quotes`]), and the per-market quote top-up would be
    /// both redundant *and* unsafe concurrently — every market's
    /// `mint_to(USDC, leader_ata, <same amount>)` is byte-identical, so
    /// parallel workers sharing a blockhash produce the same signature and all
    /// but one are dropped by the validator's dedup. The sequential caller uses
    /// [`Funding::MintBoth`]: the work between its identical mints advances the
    /// blockhash, so their signatures differ.
    MintBaseOnly,
    /// Mainnet: the leader already holds both legs, and nothing is minted —
    /// the mints are real and the admin is not their authority. The balances
    /// are checked before anything is sent.
    LeaderHeld,
}

/// Bring the market's freshly-created vault up: set the quote ladder, then
/// fund the leader's ATAs (from the admin mint authority) and seed the vault
/// with `deposit_leader`. The `leader` must be `config`'s leader key — it
/// co-signs each instruction as the vault's quote authority / leader, while
/// `wallet` (admin) pays the fees.
///
/// No reference price is stamped here, so the market opens **dark**: the
/// ladder and the inventory are in place, but the vault fails the program's
/// `has_valid_reference_price` gate, so matching skips it and the book is
/// empty until a maker bot stands behind it and stamps the first live quote.
///
/// That is deliberate, not an omission. A resting, matchable book with no
/// process refreshing it is exactly the state the maker's stale-quote
/// invalidation exists to destroy: on startup the bot compares its per-market
/// quote-state record against the staleness window, finds no age it can vouch
/// for, and stamps `price = 0`. A reference stamped here therefore made the
/// whole book vanish the moment that market's bot arrived — and again on
/// every return to a demo left sitting longer than the window. Opening dark
/// aligns the bootstrap with that invariant rather than defeating it, and
/// restores the demo's intended arc: an FX pair with no liquidity, filling in
/// live as the maker starts quoting.
///
/// `vault_idx` is the sector the caller found the leader's vault in, read
/// fresh from the chain — never assumed. Sector indices recycle, so on a
/// market anyone else has used the leader's vault need not be sector 0.
///
/// `funding` says where the deposit comes from; see [`Funding`].
#[allow(clippy::too_many_arguments)]
pub fn seed_vault(
    client: &RpcClient,
    wallet: &Keypair,
    leader: &Keypair,
    config: &PairConfig,
    market: &MarketView,
    vault_idx: u32,
    funding: Funding,
    log: &Logger,
) -> Result<()> {
    // 1. Quote ladder — a multi-rung symmetric ladder at the default spread, so
    //    the maker's first quote fans the book out across several price levels
    //    (not the single rung it would otherwise start from) at the tighter
    //    default spread. Shape only: it rests inert, pricing nothing, until a
    //    reference price is stamped.
    log.log("set_liquidity_profile");
    let bytes = ladder_profile_bytes(
        &ladder_at_spread_bps(DEFAULT_SPREAD_BPS),
        config.expiry_offset_secs,
    );
    // Check the leader can cover the deposit before the first send, so an
    // underfunded mainnet leader is refused with nothing written rather than
    // left with a ladder and no inventory.
    let (base_atoms, quote_atoms) = seed_deposit(config);
    if funding == Funding::LeaderHeld {
        for (mint, need, symbol) in [
            (&market.base_mint, base_atoms, config.base.symbol),
            (&market.quote_mint, quote_atoms, config.quote.symbol),
        ] {
            let held = chain::token_balance(client, &leader.pubkey(), mint)
                .with_context(|| format!("read the leader's {symbol} balance"))?;
            if held < need {
                anyhow::bail!(
                    "leader {} holds {held} {symbol} atoms, the deposit needs {need} — \
                     fund it first; nothing was sent",
                    leader.pubkey()
                );
            }
        }
    }
    let ix = set_liquidity_profile_ix(leader.pubkey(), market.address, vault_idx, bytes);
    chain::send_logged(
        client,
        wallet,
        &[wallet, leader],
        &[ix],
        "set_liquidity_profile",
        log,
    )
    .context("set_liquidity_profile")?;

    // 2. Fund the leader's ATAs (admin is the mint authority), then seed. The
    //    base leg is minted under either `Mint*` mode — each market's base
    //    mint / amount is unique, so it can't collide across parallel workers.
    //    The quote leg is minted only under `MintBoth`: see `Funding`.
    if funding != Funding::LeaderHeld {
        let base_ata =
            chain::create_ata_idempotent(client, wallet, &leader.pubkey(), &market.base_mint)
                .context("leader base ATA")?;
        chain::mint_to(client, wallet, &market.base_mint, &base_ata, base_atoms)
            .context("mint base to leader")?;
    }
    if funding == Funding::MintBoth {
        let quote_ata =
            chain::create_ata_idempotent(client, wallet, &leader.pubkey(), &market.quote_mint)
                .context("leader quote ATA")?;
        chain::mint_to(client, wallet, &market.quote_mint, &quote_ata, quote_atoms)
            .context("mint quote to leader")?;
    }
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
        vault_idx,
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
/// `(offset_ppm, size_bps)` list per side, sharing `expiry_secs` — to the
/// `set_liquidity_profile` argument. Rungs past the profile's capacity
/// are dropped. The symmetric [`ladder_profile_bytes`] is the common case; the
/// "thin the far side" reshape uses the asymmetric form (a full bid ladder over
/// a thinned ask ladder), so the book's shape is a direct readout of both
/// sides' rungs.
pub fn ladder_profile_bytes_asym(
    bids: &[(u32, u16)],
    asks: &[(u32, u16)],
    expiry_secs: WallSpan,
) -> [u8; PROFILE_BYTES] {
    let mut profile = LiquidityProfile::zeroed();
    for (i, &(offset_ppm, size_bps)) in bids.iter().take(profile.bids.len()).enumerate() {
        profile.bids[i].price_offset = offset_ppm.into();
        profile.bids[i].size_bps = size_bps.into();
        profile.bids[i].expiry_offset_secs = expiry_secs.get().into();
        profile.bids[i].expiry_offset_slots = SlotSpan::UNBOUNDED.get().into();
    }
    for (i, &(offset_ppm, size_bps)) in asks.iter().take(profile.asks.len()).enumerate() {
        profile.asks[i].price_offset = offset_ppm.into();
        profile.asks[i].size_bps = size_bps.into();
        profile.asks[i].expiry_offset_secs = expiry_secs.get().into();
        profile.asks[i].expiry_offset_slots = SlotSpan::UNBOUNDED.get().into();
    }
    profile_bytes(&profile)
}

/// Serialize a symmetric multi-rung ladder — the same `(offset_ppm, size_bps)`
/// list on both sides — to the `set_liquidity_profile` argument. The
/// bootstrap seeds the opening book with [`SEED_LADDER`] through here (so it
/// opens with several price levels of depth), and the TUI's widen / tighten /
/// reset reshapes all encode a full multi-level ladder through here too.
pub fn ladder_profile_bytes(rungs: &[(u32, u16)], expiry_secs: WallSpan) -> [u8; PROFILE_BYTES] {
    ladder_profile_bytes_asym(rungs, rungs, expiry_secs)
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
    use dropset_sdk::price::Price;
    use dropset_util::decimals::human_to_atoms_ratio;

    /// Every market's reference price must encode as a `Price` once scaled to
    /// the atoms-ratio — the wide unit-price spread (EURC ~$1.14 down to IDRX
    /// ~$0.000056) plus mixed decimals all has to land inside the codec range.
    /// The bootstrap doesn't stamp it, but the maker bot stamps ≈ this ratio,
    /// so a pair that fell outside the range would be permanently unquotable.
    ///
    /// Scaled through the same shared helper the maker stamps with, so this
    /// cannot range-check a formula the maker no longer uses — the drift a
    /// local copy of the conversion had made possible.
    #[test]
    fn every_market_reference_encodes() {
        for c in PAIRS {
            let ratio = human_to_atoms_ratio(c.reference_price, c.base.decimals, c.quote.decimals);
            assert!(
                Price::from_value(ratio).is_some(),
                "{} ratio {ratio} out of Price range",
                c.base.symbol
            );
        }
    }

    /// The decimals the bootstrap creates each mock mint with must equal the
    /// *mainnet* decimals the frontend scales by.
    ///
    /// This crate is the authority on what a localnet mint actually is — these
    /// values reach `spl_token`'s `InitializeMint`. The frontend, meanwhile,
    /// substitutes only the mint address and token program on localnet (its
    /// `currencies.localnet.json` overlay carries no decimals), so it keeps
    /// scaling displayed prices and the submitted swap amount by the mainnet
    /// figure. If the two disagree the demo mis-scales silently: the program
    /// hands the mint's own decimals to the checked transfer, so no CPI can
    /// notice, and a six-versus-six pair — most of this roster — hides the
    /// drift completely.
    ///
    /// Also pins that every pair *has* an overlay entry, since a missing one
    /// would leave the app addressing the real mainnet mint on localnet.
    #[test]
    fn mint_decimals_match_the_frontend_currency_data() {
        use std::collections::HashMap;

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../frontend/lib/data");
        let read = |name: &str| -> serde_json::Value {
            let path = format!("{dir}/{name}");
            let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
        };
        let currencies = read("currencies.json");
        let localnet = read("currencies.localnet.json");

        let mut mainnet_decimals: HashMap<String, u64> = HashMap::new();
        for entry in currencies
            .as_object()
            .expect("currencies.json is a currency-keyed object")
            .values()
        {
            for coin in entry["stablecoins"]
                .as_array()
                .expect("each currency carries a stablecoins array")
            {
                mainnet_decimals.insert(
                    coin["symbol"]
                        .as_str()
                        .expect("symbol is a string")
                        .to_owned(),
                    coin["decimals"].as_u64().expect("decimals is a number"),
                );
            }
        }

        let check = |spec: &MintSpec| {
            let listed = mainnet_decimals
                .get(spec.symbol)
                .copied()
                .unwrap_or_else(|| {
                    panic!(
                        "{} is not listed in the frontend's currencies.json",
                        spec.symbol
                    )
                });
            assert_eq!(
                listed,
                u64::from(spec.decimals),
                "{}: frontend scales by {listed} decimals, bootstrap creates the \
                 mint with {}",
                spec.symbol,
                spec.decimals
            );
            assert!(
                localnet.get(spec.symbol).is_some(),
                "{} has no entry in currencies.localnet.json, so the frontend \
                 would address its mainnet mint on localnet",
                spec.symbol
            );
        };

        for c in PAIRS {
            check(&c.base);
            check(&c.quote);
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
            let LeaderKey::Keypair(leader) = c.leader else {
                panic!("{}: a localnet pair names its leader file", c.base.symbol);
            };
            assert_ne!(MintKey::Keypair(leader), c.base.key);
            assert_ne!(MintKey::Keypair(leader), c.quote.key);
            assert_eq!(c.quote.key, MintKey::Keypair("keys/USDC.json"));
        }
    }

    /// The mainnet roster must never be able to mint or sign with a committed
    /// key: every mint is an existing address and every leader comes from the
    /// operator. This is the by-construction half of "real mints are never
    /// created" — `ensure_mint` cannot reach `create_mint` for an
    /// `Existing` mint.
    #[test]
    fn mainnet_roster_references_real_mints_and_operator_leaders() {
        let symbols = |c: Cluster| roster(c).iter().map(|p| p.base.symbol).collect::<Vec<_>>();
        assert_eq!(symbols(Cluster::Mainnet), ["EURC", "AUDD", "CADC"]);
        assert_eq!(symbols(Cluster::Localnet).len(), PAIRS.len());
        for c in MAINNET_PAIRS {
            assert!(
                matches!(c.base.key, MintKey::Existing(_)),
                "{}",
                c.base.symbol
            );
            assert_eq!(c.quote.key, MintKey::Existing(MAINNET_USDC));
            assert_eq!(c.leader, LeaderKey::Operator);
        }
        // And the localnet roster stays entirely keypair-backed, so the
        // bootstrap never reaches a real issuer's address.
        for c in PAIRS {
            assert!(matches!(c.base.key, MintKey::Keypair(_)));
            assert!(matches!(c.quote.key, MintKey::Keypair(_)));
        }
    }

    /// A pair whose leader is operator-supplied refuses to load without one,
    /// instead of quietly falling back to a committed role key.
    #[test]
    fn operator_leader_is_required_on_mainnet() {
        let root = Path::new("/nonexistent");
        let err = leader(root, &MAINNET_EURC, None).unwrap_err();
        assert!(format!("{err:#}").contains("--leader"), "{err:#}");
        let supplied = Keypair::new();
        let got = leader(root, &MAINNET_EURC, Some(&supplied)).unwrap();
        assert_eq!(got.pubkey(), supplied.pubkey());
    }

    /// The Rust-side mainnet addresses are a second copy of the frontend's
    /// `currencies.json` — the one place they lived before — so pin the two
    /// equal: address and decimals, base and quote. A typo here would point a
    /// real-funds ceremony at the wrong mint, which `verify_mint` would only
    /// catch if the wrong address happened not to be a mint at all.
    #[test]
    fn mainnet_mints_match_the_frontend_currency_data() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../frontend/lib/data/currencies.json"
        );
        let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let currencies: serde_json::Value =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let listed = |symbol: &str| -> (String, u64) {
            currencies
                .as_object()
                .expect("currencies.json is a currency-keyed object")
                .values()
                .flat_map(|entry| entry["stablecoins"].as_array().cloned().unwrap_or_default())
                .find(|coin| coin["symbol"] == symbol)
                .map(|coin| {
                    (
                        coin["mint"].as_str().expect("mint is a string").to_owned(),
                        coin["decimals"].as_u64().expect("decimals is a number"),
                    )
                })
                .unwrap_or_else(|| panic!("{symbol} is not listed in currencies.json"))
        };
        for c in MAINNET_PAIRS {
            for spec in [&c.base, &c.quote] {
                let MintKey::Existing(address) = spec.key else {
                    unreachable!("pinned by the roster test");
                };
                let (mint, decimals) = listed(spec.symbol);
                assert_eq!(address.to_string(), mint, "{} address", spec.symbol);
                assert_eq!(
                    u64::from(spec.decimals),
                    decimals,
                    "{} decimals",
                    spec.symbol
                );
            }
        }
    }

    /// The mainnet pairs reuse the localnet seed sizing, so the same two
    /// invariants must hold for them: a quotable reference and a balanced
    /// ≈ $100-a-side deposit.
    #[test]
    fn mainnet_pairs_encode_and_seed_balanced() {
        for c in MAINNET_PAIRS {
            let ratio = human_to_atoms_ratio(c.reference_price, c.base.decimals, c.quote.decimals);
            assert!(Price::from_value(ratio).is_some(), "{}", c.base.symbol);
            let (base_atoms, quote_atoms) = seed_deposit(c);
            let base_usd =
                base_atoms as f64 / 10f64.powi(c.base.decimals as i32) * c.reference_price;
            assert!(
                (base_usd - SEED_USD_PER_SIDE).abs() < 1.0,
                "{}",
                c.base.symbol
            );
            assert_eq!(quote_atoms, 100_000_000, "{}", c.base.symbol);
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
        let bytes = ladder_profile_bytes_asym(&SEED_LADDER, &asks, WallSpan::UNBOUNDED);
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
        let bytes = ladder_profile_bytes(&SEED_LADDER, WallSpan::UNBOUNDED);
        assert_eq!(bytes.len(), PROFILE_BYTES);
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
