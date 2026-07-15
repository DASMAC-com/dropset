//! Bot configuration — the knobs `docs/market-making-mvp.md` pins down.
//!
//! Defaults encode the MVP spec verbatim: the tiered price feed and poll
//! cadences (§1), the 50/100/200/500 bps ladder (§2), the `SetReferencePrice`
//! / `SetLiquidityProfile` triggers (§3), the linear inventory skew (§2), and
//! the inventory / peg / staleness kill-switch bounds (§4). Secrets (the
//! CoinMarketCap API key) come from the environment, never a committed field —
//! the same convention the Linear tooling uses.
//!
//! The demo runs **many** FX-stablecoin markets at once, all quoted against
//! USDC ([`MARKETS`]). Each carries the per-tier feed identifiers — its
//! CoinGecko id, its (optional) CoinMarketCap numeric id, and the ISO 4217
//! currency the keyless FX-rate tier pegs to — plus the mock-mint keypair and
//! decimals the localnet bootstrap and inventory valuation need.

use dropset_fair_value::FairValueConfig;
use std::time::Duration;

/// Default localnet RPC endpoint (the `solana-test-validator` the TUI spawns).
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";

/// The vault the bootstrap opens first; the bot quotes this sector.
pub const DEFAULT_VAULT_IDX: u32 = 0;

/// The leader / quote-authority role key the localnet bootstrap seeds every
/// mock vault with (`tui/src/market.rs`). The bot signs the vault-gated
/// hot/cold path with it. On localnet one leader quotes all markets; the
/// delegated per-market `quote_authority` model is the devnet/mainnet
/// promotion's concern, not this localnet plumbing.
pub const DEFAULT_LEADER_KEY: &str = "keys/EEEE.json";

/// The shared quote mint — every demo market is `<token>/USDC`. The mock
/// localnet USDC mint keypair and its decimals.
pub const QUOTE_KEYPAIR_FILE: &str = "keys/USDC.json";
pub const QUOTE_DECIMALS: u8 = 6;

/// CoinGecko id for USDC, priced in USD — the USDC/USD common-mode leg (§1
/// fm1). One id shared by every market (all quote against the same USDC), so it
/// rides the existing batched CoinGecko call rather than a per-market lookup.
pub const USDC_COINGECKO_ID: &str = "usd-coin";

/// One FX-stablecoin market: a base token quoted against USDC, with the
/// per-tier feed identifiers and the mint / decimals the bot needs to address
/// its vault and value inventory. The reference price is *discovered* from the
/// feeds, so — unlike the bootstrap's `PairConfig` — no seed price lives here.
#[derive(Clone, Copy, Debug)]
pub struct MarketConfig {
    /// Human ticker, for logs and to map a discovered market back to its feeds.
    pub symbol: &'static str,
    /// The mock base-mint keypair (relative to the repo root); its pubkey,
    /// paired with USDC, seeds the market PDA the bot discovers.
    pub base_keypair_file: &'static str,
    /// Base-mint decimals — matched to the real token so the localnet plumbing
    /// exercises the same per-market decimal/atoms-ratio path mainnet will.
    pub base_decimals: u8,
    /// ISO 4217 code of the fiat the token tracks — the symbol the keyless
    /// ECB/Frankfurter FX-rate tier and the static last-resort peg to.
    pub currency: &'static str,
    /// CoinGecko coin id (primary tier, batched `/simple/price`).
    pub coingecko_id: &'static str,
    /// CoinMarketCap numeric id (secondary tier, batched by id). `None` for a
    /// token CMC doesn't list (MXNe), which simply skips the CMC tier and
    /// cascades CoinGecko → FX-rate → static.
    pub coinmarketcap_id: Option<u32>,
    /// Last-resort static USD-per-token peg, used when every live feed is down.
    /// A representative spot value; the FX-rate tier supersedes it whenever the
    /// keyless ECB feed answers.
    pub static_usd: f64,
}

/// The demo roster — the seven non-USD FX stablecoins with ≥ $1k Solana
/// liquidity, each quoted against USDC at $100 top-of-book. The CoinGecko ids
/// are from a by-contract lookup on each token's real mainnet mint; the
/// CoinMarketCap ids from its `cryptocurrency/detail` record. MXNe (Real MXN)
/// is not listed on CoinMarketCap, so its secondary tier is `None`.
pub const MARKETS: [MarketConfig; 7] = [
    MarketConfig {
        symbol: "EURC",
        base_keypair_file: "keys/EURC.json",
        base_decimals: 6,
        currency: "EUR",
        coingecko_id: "euro-coin",
        coinmarketcap_id: Some(20641),
        static_usd: 1.14,
    },
    MarketConfig {
        symbol: "VCHF",
        base_keypair_file: "keys/VCHF.json",
        base_decimals: 9,
        currency: "CHF",
        coingecko_id: "vnx-swiss-franc",
        coinmarketcap_id: Some(24130),
        static_usd: 1.235,
    },
    MarketConfig {
        symbol: "TGBP",
        base_keypair_file: "keys/TGBP.json",
        base_decimals: 9,
        currency: "GBP",
        coingecko_id: "tokenised-gbp",
        coinmarketcap_id: Some(38935),
        static_usd: 1.324,
    },
    MarketConfig {
        symbol: "ZARP",
        base_keypair_file: "keys/ZARP.json",
        base_decimals: 6,
        currency: "ZAR",
        coingecko_id: "zarp-stablecoin",
        coinmarketcap_id: Some(21856),
        static_usd: 0.0605,
    },
    MarketConfig {
        symbol: "MXNe",
        base_keypair_file: "keys/MXNe.json",
        base_decimals: 9,
        currency: "MXN",
        coingecko_id: "real-mxn",
        coinmarketcap_id: None,
        static_usd: 0.0573,
    },
    MarketConfig {
        symbol: "XSGD",
        base_keypair_file: "keys/XSGD.json",
        base_decimals: 6,
        currency: "SGD",
        coingecko_id: "xsgd",
        coinmarketcap_id: Some(8489),
        static_usd: 0.7705,
    },
    MarketConfig {
        symbol: "IDRX",
        base_keypair_file: "keys/idrx.json",
        base_decimals: 2,
        currency: "IDR",
        coingecko_id: "idrx",
        coinmarketcap_id: Some(26732),
        static_usd: 0.000056,
    },
];

/// One rung of the quote ladder: a ppm offset from the reference price, a
/// fraction of the inventory leg in bps, and a per-level expiry in slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LadderLevel {
    /// Offset from `reference_price`, in ppm (bids subtract, asks add).
    pub offset_ppm: u32,
    /// Fraction of the inventory leg, in bps (10000 = 100%).
    pub size_bps: u16,
    /// Slots after the quote's `quote_slot` at which this level expires.
    pub expiry_offset: u32,
}

/// The spec's hand-shaped ladder (§2 + the §3 expiry table): top-of-book at
/// 50 bps taking 40% of the leg and expiring fastest, widening and shrinking
/// to a 500 bps tail that lives ~50 min. `Σ size_bps = 10000` per side.
pub const DEFAULT_LADDER: [LadderLevel; 4] = [
    LadderLevel {
        offset_ppm: 5_000,
        size_bps: 4_000,
        expiry_offset: 90,
    },
    LadderLevel {
        offset_ppm: 10_000,
        size_bps: 3_000,
        expiry_offset: 300,
    },
    LadderLevel {
        offset_ppm: 20_000,
        size_bps: 2_000,
        expiry_offset: 1_200,
    },
    LadderLevel {
        offset_ppm: 50_000,
        size_bps: 1_000,
        expiry_offset: 7_200,
    },
];

/// The tiered price feed (§1): poll cadences and base URLs for the four
/// sources `fair_mid` cascades through, primary-first. The per-token
/// identifiers live on each [`MarketConfig`]; only the transport settings are
/// here. Base URLs are fields so tests can point them at a local stub. The
/// CoinMarketCap API key is read from `CMC_API_KEY` at run time, never a field.
#[derive(Clone, Debug)]
pub struct FeedConfig {
    /// CoinGecko poll interval (primary). One batched `/simple/price` call
    /// covers every market, so 10 s stays well under the free-tier ceiling.
    pub coingecko_poll: Duration,
    /// CoinMarketCap poll interval (secondary). Polled only when CoinGecko is
    /// down or throttled — the free tier's ~10k/mo quota rules out a hot poll —
    /// so this is the *minimum* spacing between fallback calls, not a cadence.
    pub coinmarketcap_poll: Duration,
    /// ECB/Frankfurter FX-rate poll interval (tertiary). ECB publishes once a
    /// working day, so a slow poll suffices.
    pub fx_poll: Duration,
    /// CoinGecko REST base URL (`/simple/price` is appended).
    pub coingecko_base_url: String,
    /// CoinMarketCap REST base URL (`/v2/cryptocurrency/quotes/latest`).
    pub coinmarketcap_base_url: String,
    /// Frankfurter REST base URL (`/latest`), the keyless ECB FX-rate feed.
    pub frankfurter_base_url: String,
}

/// Quoting strategy parameters (§2–§3).
#[derive(Clone, Debug)]
pub struct StrategyConfig {
    /// The quote ladder.
    pub ladder: Vec<LadderLevel>,
    /// Linear inventory skew: shift the reference by this many bps per 1% of
    /// TVL of signed inventory deviation (§2 override of the formal A-S skew).
    /// Keyed to fractional deviation so one calibration holds at any vault
    /// size — see the module header for why.
    pub skew_bps_per_pct_tvl: f64,
    /// Cap on the inventory skew, in bps (±).
    pub skew_cap_bps: f64,
    /// `SetReferencePrice` price-drift trigger: refresh when `fair_mid` moves
    /// this many bps from the last set price (§3).
    pub ref_drift_bps: f64,
    /// `SetReferencePrice` heartbeat: refresh at least this often (§3).
    pub ref_heartbeat: Duration,
    /// `SetReferencePrice` skew trigger: refresh when the inventory skew shifts
    /// the reference by more than this many bps (§3).
    pub ref_skew_change_bps: f64,
    /// `SetLiquidityProfile` daily heartbeat — re-arm the ladder at least this
    /// often so deep, rarely-filled levels don't expire dark (§3 cold-path
    /// trigger 3).
    pub profile_heartbeat: Duration,
    /// §4 reshape (imbalance > 30%): the fraction the *accumulating* side's
    /// `size_bps` is scaled to. The heavy (rebuild) side stays at full commit,
    /// so it dominates the book and leans into offloading the heavy leg — the
    /// realizable form of "grow the heavy side" given the `Σ size_bps = 10000`
    /// per-side invariant.
    pub reshape_accumulating_scale: f64,
}

/// Inventory / TVL kill-switch bounds (§4).
///
/// The fair-value guards (the basis band, the USDC/USD common-mode band, and
/// per-leg staleness) moved to [`FairValueConfig`], which the engine evaluates;
/// the breaches arrive here as flags on the composed reference. What stays are
/// the inventory-imbalance ladder and the TVL drawdown floor.
#[derive(Clone, Copy, Debug)]
pub struct KillSwitchConfig {
    /// Per-side imbalance (% off the 50/50 launch split) that triggers a cold
    /// reshape (§4 row 1, §3 cold-path trigger).
    pub imbalance_reshape_pct: f64,
    /// Imbalance that freezes the heavy side (§4 row 2).
    pub imbalance_freeze_side_pct: f64,
    /// Imbalance that halts the whole vault for review (§4 row 3).
    pub imbalance_halt_pct: f64,
    /// TVL floor that halts the vault for post-mortem (§4 last row), as a
    /// *fraction of launch TVL* — `0.8` halts on a 20% drawdown. Launch TVL is
    /// read from the vault at startup (not a config constant), so the floor
    /// self-scales per market — see the module header.
    pub tvl_floor_frac: f64,
}

/// The full bot configuration.
#[derive(Clone, Debug)]
pub struct BotConfig {
    /// RPC endpoint.
    pub rpc_url: String,
    /// PubSub websocket endpoint for the fill-event subscription. `None`
    /// derives it from `rpc_url` via
    /// [`dropset_localnet_support::ws_url_from_rpc`].
    pub ws_url: Option<String>,
    /// Vault sector the bot quotes.
    pub vault_idx: u32,
    /// Bot tick interval — the §3 5-second heartbeat.
    pub tick: Duration,
    pub feeds: FeedConfig,
    pub strategy: StrategyConfig,
    pub kill: KillSwitchConfig,
    /// The fair-value engine's calibration (`fair = fx × basis`, §1). Almost
    /// every value is a survey-set placeholder — see [`FairValueConfig`].
    pub fair_value: FairValueConfig,
}

/// Environment variable holding the CoinMarketCap API key (never committed).
pub const CMC_KEY_ENV: &str = "CMC_API_KEY";

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            // CoinGecko's keyless tier rate-limits by IP, and the localnet demo
            // runs one maker process per market — so seven processes share that
            // budget. A 60 s base, plus the on-failure exponential backoff in
            // `tasks.rs`, keeps the aggregate request rate well under the limit;
            // the FX-rate / static tiers cover any gaps. (The definitive fix is
            // one maker process for the whole roster — one batched call — but
            // that trades away the per-market start/stop the demo uses.)
            coingecko_poll: Duration::from_secs(60),
            coinmarketcap_poll: Duration::from_secs(60),
            fx_poll: Duration::from_secs(300),
            coingecko_base_url: "https://api.coingecko.com/api/v3".to_string(),
            coinmarketcap_base_url: "https://pro-api.coinmarketcap.com".to_string(),
            frankfurter_base_url: "https://api.frankfurter.dev/v1".to_string(),
        }
    }
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            ladder: DEFAULT_LADDER.to_vec(),
            skew_bps_per_pct_tvl: 0.5,
            skew_cap_bps: 20.0,
            ref_drift_bps: 10.0,
            ref_heartbeat: Duration::from_secs(30),
            ref_skew_change_bps: 2.0,
            profile_heartbeat: Duration::from_secs(24 * 3600),
            reshape_accumulating_scale: 0.5,
        }
    }
}

impl Default for KillSwitchConfig {
    fn default() -> Self {
        Self {
            imbalance_reshape_pct: 30.0,
            imbalance_freeze_side_pct: 50.0,
            imbalance_halt_pct: 80.0,
            tvl_floor_frac: 0.8,
        }
    }
}

impl Default for BotConfig {
    fn default() -> Self {
        Self {
            rpc_url: DEFAULT_RPC_URL.to_string(),
            ws_url: None,
            vault_idx: DEFAULT_VAULT_IDX,
            tick: Duration::from_secs(5),
            feeds: FeedConfig::default(),
            strategy: StrategyConfig::default(),
            kill: KillSwitchConfig::default(),
            fair_value: FairValueConfig {
                // The bot's FX anchor is currently the daily Frankfurter feed,
                // polled every `fx_poll` (300 s); a staleness bound comfortably
                // above that keeps the slow anchor from flapping in and out of
                // the composition each cycle. TBD(survey): split per leg so the
                // fast crypto leg can be checked on a tighter bound.
                leg_stale: Duration::from_secs(15 * 60),
                ..FairValueConfig::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default ladder commits exactly the full leg per side (§2 invariant
    /// `Σ size_bps = 10000`) and stays within the 8-level cap.
    #[test]
    fn default_ladder_commits_full_leg() {
        let total: u32 = DEFAULT_LADDER.iter().map(|l| l.size_bps as u32).sum();
        assert_eq!(total, 10_000);
        assert!(DEFAULT_LADDER.len() <= 8);
    }

    /// Top-of-book is the tightest offset and expires fastest; the tail is
    /// widest and longest-lived (§3 expiry stratification).
    #[test]
    fn ladder_is_monotonic() {
        for w in DEFAULT_LADDER.windows(2) {
            assert!(w[1].offset_ppm > w[0].offset_ppm);
            assert!(w[1].size_bps < w[0].size_bps);
            assert!(w[1].expiry_offset > w[0].expiry_offset);
        }
    }

    /// Every demo market names a base mint, a CoinGecko id, a tracked
    /// currency, and a positive static peg; symbols and mint files are unique
    /// so the roster maps cleanly onto distinct vaults.
    #[test]
    fn markets_roster_is_well_formed() {
        use std::collections::HashSet;
        let mut symbols = HashSet::new();
        let mut files = HashSet::new();
        for m in MARKETS {
            assert!(!m.symbol.is_empty());
            assert!(!m.coingecko_id.is_empty());
            assert_eq!(m.currency.len(), 3, "{} currency is ISO 4217", m.symbol);
            assert!(m.static_usd > 0.0, "{} static peg", m.symbol);
            assert!(m.base_decimals <= 9, "{} decimals", m.symbol);
            assert!(symbols.insert(m.symbol), "duplicate symbol {}", m.symbol);
            assert!(
                files.insert(m.base_keypair_file),
                "duplicate mint file {}",
                m.base_keypair_file
            );
        }
    }
}
