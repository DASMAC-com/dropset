// cspell:word EURCUSD
// cspell:word hexdigit
//! Bot configuration — the knobs `docs/market-making.md` pins down.
//!
//! Defaults encode the spec verbatim: the tiered price feed and poll
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
use dropset_sdk::quoting::NO_SLOT_BOUND;
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

/// Kraken pair for USDC against USD — the **primary** USDC/USD common-mode leg
/// (§1 fm1). One pair shared by every market, so it rides the batched Kraken
/// call rather than a per-market lookup.
///
/// Kraken is the venue for this reading because the alternatives do not quote
/// it honestly: Coinbase Exchange lists no `USDC-USD` product at all, and
/// Binance.US carries the pair but prints an administered flat `1.00000000`,
/// which would report a depeg as perfect health.
pub const USDC_KRAKEN_PAIR: &str = "USDCUSD";

/// CoinGecko id for USDC, priced in USD — the USDC/USD common-mode leg's
/// **fallback**, used when the Kraken reading is absent or stale. It is a
/// derived index rather than a venue print, which is why it was demoted.
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
    /// ECB/Frankfurter FX anchor and the static last-resort peg to.
    pub currency: &'static str,
    /// Pyth Hermes feed id for this market's fiat cross — the **primary** FX
    /// anchor, which unlike Frankfurter publishes a confidence half-width.
    pub pyth_feed_id: &'static str,
    /// Whether [`MarketConfig::pyth_feed_id`] is published as `USD/<ccy>` and
    /// must be reciprocated into USD per `<ccy>`. Pyth quotes each cross one
    /// way only, and for five of the seven roster currencies that is the
    /// inverted direction.
    pub pyth_invert: bool,
    /// Coinbase product id for the token against USDC — the **primary** basis
    /// leg, in exactly the engine's units. `None` for a token Coinbase doesn't
    /// list, which is all of the roster but EURC.
    pub coinbase_product: Option<&'static str>,
    /// Kraken pair name for the token against USD — the basis-leg secondary.
    /// `None` for a token Kraken doesn't list (every exotic).
    pub kraken_pair: Option<&'static str>,
    /// CoinGecko coin id — the crypto basis-leg fallback (batched
    /// `/simple/price`). Load-bearing for the six exotics no CEX quotes.
    pub coingecko_id: &'static str,
    /// CoinMarketCap numeric id — the last basis-leg fallback (batched by id).
    /// `None` for a token CMC doesn't list (MXNe), which simply leaves
    /// CoinGecko as the sole crypto basis source there.
    pub coinmarketcap_id: Option<u32>,
    /// Last-resort static USD-per-token peg, used only when every live leg is
    /// down. A representative spot value; a live FX anchor and basis supersede
    /// it whenever the feeds answer.
    pub static_usd: f64,
}

/// The demo roster — the seven non-USD FX stablecoins with ≥ $1k Solana
/// liquidity, each quoted against USDC at $100 top-of-book. The CoinGecko ids
/// are from a by-contract lookup on each token's real mainnet mint; the
/// CoinMarketCap ids from its `cryptocurrency/detail` record. MXNe (Real MXN)
/// is not listed on CoinMarketCap, so that tier is `None`.
///
/// The Pyth feed ids are from the Hermes FX catalogue
/// (`/v2/price_feeds?asset_type=fx`). Only EUR and GBP are published as
/// `<ccy>/USD`; the rest are `USD/<ccy>` and carry `pyth_invert: true`.
///
/// **Only EURC reaches a CEX.** Coinbase lists `EURC-USDC` and Kraken lists
/// `EURC/USD`; none of the other six tokens trades on either venue, so their
/// basis leg has no primary tier and the CoinGecko / CoinMarketCap fallbacks
/// carry it. That asymmetry is the roster's, not a gap in the wiring.
pub const MARKETS: [MarketConfig; 7] = [
    MarketConfig {
        symbol: "EURC",
        base_keypair_file: "keys/EURC.json",
        base_decimals: 6,
        currency: "EUR",
        pyth_feed_id: "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
        pyth_invert: false,
        coinbase_product: Some("EURC-USDC"),
        kraken_pair: Some("EURCUSD"),
        coingecko_id: "euro-coin",
        coinmarketcap_id: Some(20641),
        static_usd: 1.14,
    },
    MarketConfig {
        symbol: "VCHF",
        base_keypair_file: "keys/VCHF.json",
        base_decimals: 9,
        currency: "CHF",
        pyth_feed_id: "0b1e3297e69f162877b577b0d6a47a0d63b2392bc8499e6540da4187a63e28f8",
        pyth_invert: true,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "vnx-swiss-franc",
        coinmarketcap_id: Some(24130),
        static_usd: 1.235,
    },
    MarketConfig {
        symbol: "TGBP",
        base_keypair_file: "keys/TGBP.json",
        base_decimals: 9,
        currency: "GBP",
        pyth_feed_id: "84c2dde9633d93d1bcad84e7dc41c9d56578b7ec52fabedc1f335d673df0a7c1",
        pyth_invert: false,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "tokenised-gbp",
        coinmarketcap_id: Some(38935),
        static_usd: 1.324,
    },
    MarketConfig {
        symbol: "ZARP",
        base_keypair_file: "keys/ZARP.json",
        base_decimals: 6,
        currency: "ZAR",
        pyth_feed_id: "389d889017db82bf42141f23b61b8de938a4e2d156e36312175bebf797f493f1",
        pyth_invert: true,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "zarp-stablecoin",
        coinmarketcap_id: Some(21856),
        static_usd: 0.0605,
    },
    MarketConfig {
        symbol: "MXNe",
        base_keypair_file: "keys/MXNe.json",
        base_decimals: 9,
        currency: "MXN",
        pyth_feed_id: "e13b1c1ffb32f34e1be9545583f01ef385fde7f42ee66049d30570dc866b77ca",
        pyth_invert: true,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "real-mxn",
        coinmarketcap_id: None,
        static_usd: 0.0573,
    },
    MarketConfig {
        symbol: "XSGD",
        base_keypair_file: "keys/XSGD.json",
        base_decimals: 6,
        currency: "SGD",
        pyth_feed_id: "396a969a9c1480fa15ed50bc59149e2c0075a72fe8f458ed941ddec48bdb4918",
        pyth_invert: true,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "xsgd",
        coinmarketcap_id: Some(8489),
        static_usd: 0.7705,
    },
    MarketConfig {
        symbol: "IDRX",
        base_keypair_file: "keys/idrx.json",
        base_decimals: 2,
        currency: "IDR",
        pyth_feed_id: "6693afcd49878bbd622e46bd805e7177932cf6ab0b1c91b135d71151b9207433",
        pyth_invert: true,
        coinbase_product: None,
        kraken_pair: None,
        coingecko_id: "idrx",
        coinmarketcap_id: Some(26732),
        static_usd: 0.000056,
    },
];

/// One rung of the quote ladder: a ppm offset from the reference price, a
/// fraction of the inventory leg in bps, and a per-level expiry in each
/// of the two domains the engine gates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LadderLevel {
    /// Offset from `reference_price`, in ppm (bids subtract, asks add).
    pub offset_ppm: u32,
    /// Fraction of the inventory leg, in bps (10000 = 100%).
    pub size_bps: u16,
    /// Seconds after the quote's `quote_unix` wall-clock datum at which
    /// this level expires.
    pub expiry_offset_secs: u32,
    /// Slots after the quote's `quote_slot` datum at which this level
    /// expires — the second, independent bound. [`NO_SLOT_BOUND`] leaves
    /// a level bounded only in wall time.
    pub expiry_offset_slots: u32,
}

/// The spec's hand-shaped ladder (§2 + the §3 expiry table): top-of-book at
/// 50 bps taking 40% of the leg and expiring fastest, widening and shrinking
/// to a 500 bps tail that lives ~48 min. `Σ size_bps = 10000` per side.
///
/// Wall expiries carry over from the table's former slot denomination at
/// the mainnet ~0.4 s/slot pace it assumed (90 / 300 / 1200 / 7200 slots
/// → 36 s / 2 min / 8 min / 48 min), so the nominal lives are unchanged;
/// what changed is that they now hold through a halt, where slots stop
/// ticking but wall time does not.
///
/// The **slot** bound is the other half of the dual gate, and it is where
/// the tight tiers get a deadline the wall domain cannot express. The
/// cluster clock is second-denominated and accurate only to a few
/// seconds, which floors any wall TIF at ~15 s — a ~37-slot dead-man tail
/// behind a prop-cadence quoter. Top-of-book therefore takes a **2-slot**
/// bound (sub-second: the level dies almost immediately unless the next
/// tick re-stamps it), widening with depth, and the deepest tier goes
/// [`NO_SLOT_BOUND`] so its stratified wall decay is what governs it.
///
/// These values are the shape of the policy, not a calibration — the
/// vol-ladder retune owns the tuning.
pub const DEFAULT_LADDER: [LadderLevel; 4] = [
    LadderLevel {
        offset_ppm: 5_000,
        size_bps: 4_000,
        expiry_offset_secs: 36,
        expiry_offset_slots: 2,
    },
    LadderLevel {
        offset_ppm: 10_000,
        size_bps: 3_000,
        expiry_offset_secs: 120,
        expiry_offset_slots: 30,
    },
    LadderLevel {
        offset_ppm: 20_000,
        size_bps: 2_000,
        expiry_offset_secs: 480,
        expiry_offset_slots: 300,
    },
    LadderLevel {
        offset_ppm: 50_000,
        size_bps: 1_000,
        expiry_offset_secs: 2_880,
        expiry_offset_slots: NO_SLOT_BOUND,
    },
];

/// The price feeds (§1): poll cadences and base URLs for the sources the
/// fair-value engine composes its legs from. The per-token identifiers live on
/// each [`MarketConfig`]; only the transport settings are here. Base URLs are
/// fields so tests can point them at a local stub. The CoinMarketCap API key
/// is read from `CMC_API_KEY` at run time, never a field.
#[derive(Clone, Debug)]
pub struct FeedConfig {
    /// CoinGecko poll interval (primary). One batched `/simple/price` call
    /// covers every market, so 10 s stays well under the free-tier ceiling.
    pub coingecko_poll: Duration,
    /// CoinMarketCap poll interval (secondary). Polled only when CoinGecko is
    /// down or throttled — the free tier's ~10k/mo quota rules out a hot poll —
    /// so this is the *minimum* spacing between fallback calls, not a cadence.
    pub coinmarketcap_poll: Duration,
    /// ECB/Frankfurter FX-anchor poll interval. ECB publishes once a working
    /// day, so a slow poll suffices.
    pub fx_poll: Duration,
    /// Pyth Hermes FX-anchor poll interval — the primary anchor tier. Hermes
    /// republishes on the order of a second, so this is the cadence at which
    /// the anchor actually moves and is polled far harder than the daily ECB
    /// fallback below.
    pub pyth_poll: Duration,
    /// Kraken poll interval — the batched basis / peg-truth tier.
    pub kraken_poll: Duration,
    /// Coinbase spot-ticker poll interval — the primary basis tier.
    pub coinbase_poll: Duration,
    /// CoinGecko REST base URL (`/simple/price` is appended).
    pub coingecko_base_url: String,
    /// CoinMarketCap REST base URL (`/v2/cryptocurrency/quotes/latest`).
    pub coinmarketcap_base_url: String,
    /// Frankfurter REST base URL (`/latest`), the keyless ECB FX-rate feed.
    pub frankfurter_base_url: String,
    /// Pyth Hermes base URL (`/v2/updates/price/latest`).
    pub pyth_base_url: String,
    /// Kraken public REST base URL (`/0/public/Ticker`).
    pub kraken_base_url: String,
    /// Coinbase Exchange REST base URL (`/products/{id}/ticker`).
    pub coinbase_base_url: String,
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
    /// What the bounds above are scaled by while the composition runs degraded
    /// — the spec's "tighten kill switches by 50%" (§4), so `0.5`. The
    /// imbalance thresholds scale directly; the TVL floor scales its *permitted
    /// drawdown*, which raises the floor toward launch TVL. See
    /// [`crate::model::killswitch`].
    pub degraded_scale: f64,
}

/// Stale-quote invalidation (the bot half of the halt / pick-off mitigation).
///
/// A resting quote stays matchable until a level deadline passes —
/// up to ~48 min on the deepest tier. That life is now wall-clock bounded
/// (levels are measured from the quote's `quote_unix` datum, so a halt no
/// longer freezes the countdown), but a staleness *cap* is not the same as
/// an unattended-book policy: 48 min of drift is still far more than the
/// bot intends to leave resting. When the bot
/// stops refreshing the reference (a restart, a chain halt, feeds gone dark),
/// these knobs govern the kill stamp that takes the book dark instead of leaving
/// it to expire. See [`crate::model::invalidate`].
#[derive(Clone, Debug)]
pub struct InvalidateConfig {
    /// A resting quote older than this is killed rather than left matchable.
    ///
    /// Twice the `SetReferencePrice` heartbeat (30 s), so a healthy bot's own
    /// last stamp is always comfortably inside the bound and an ordinary restart
    /// doesn't churn the book — while still being ~2% of the deepest ladder
    /// tier's ~48 min wall-clock life, so no level ever rests unattended for
    /// more than about a minute.
    pub stale_after: Duration,
    /// Priority fee for the kill stamp, in micro-lamports per compute unit.
    ///
    /// The invalidation races takers in the first blocks after the bot comes
    /// back, so it does not queue behind them at the base fee; losing that race
    /// by a block is the residual exposure this mitigation accepts. Only the
    /// unit *price* is set, not a unit limit — a limit guessed under the
    /// handler's actual cost would fail the stamp outright, which is the one
    /// outcome worse than paying for a few unused units. The value is a
    /// localnet placeholder; a mainnet promotion wants it fee-market-aware.
    pub priority_micro_lamports: u64,
    /// Directory holding the per-market last-live-stamp records
    /// ([`crate::quote_state`]).
    pub state_dir: String,
}

/// The full bot configuration.
#[derive(Clone, Debug)]
pub struct BotConfig {
    /// RPC endpoint.
    pub rpc_url: String,
    /// PubSub websocket endpoint for the fill-event subscription. `None`
    /// derives it from `rpc_url` via
    /// [`dropset_util::rpc::ws_url_from_rpc`].
    pub ws_url: Option<String>,
    /// Vault sector the bot quotes.
    pub vault_idx: u32,
    /// Bot tick interval — the §3 5-second heartbeat.
    pub tick: Duration,
    pub feeds: FeedConfig,
    pub strategy: StrategyConfig,
    pub kill: KillSwitchConfig,
    pub invalidate: InvalidateConfig,
    /// The fair-value engine's calibration (`fair = fx × basis`, §1). Almost
    /// every value is an analytics-set placeholder — see [`FairValueConfig`].
    pub fair_value: FairValueConfig,
}

/// Environment variable holding the CoinMarketCap API key (never committed).
pub const CMC_KEY_ENV: &str = "CMC_API_KEY";

/// The CoinMarketCap API key for this run, or `None` when that tier is not
/// wired up (the localnet demo runs without it).
///
/// This lives with the bot's configuration, not with the venue adapter it
/// feeds: `dropset_feeds::venues::CmcSource` takes its key as an argument, so
/// *where a secret comes from* stays a deployment decision the app owns. That
/// is what lets the same adapter later be handed a key from a secrets provider
/// without touching it.
pub fn cmc_api_key() -> Option<String> {
    std::env::var(CMC_KEY_ENV).ok().filter(|k| !k.is_empty())
}

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
            // The primaries are keyless but not rate-limit-free, and one poll
            // covers the whole roster in each case. A 5 s Hermes cadence tracks
            // the anchor at the bot's own tick rate; the CEX basis legs move
            // slowly enough (a pegged token against its peg) that 15 s is ample.
            pyth_poll: Duration::from_secs(5),
            kraken_poll: Duration::from_secs(15),
            coinbase_poll: Duration::from_secs(15),
            coingecko_base_url: "https://api.coingecko.com/api/v3".to_string(),
            coinmarketcap_base_url: "https://pro-api.coinmarketcap.com".to_string(),
            frankfurter_base_url: "https://api.frankfurter.dev/v1".to_string(),
            pyth_base_url: "https://hermes.pyth.network".to_string(),
            kraken_base_url: "https://api.kraken.com".to_string(),
            coinbase_base_url: "https://api.exchange.coinbase.com".to_string(),
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
            degraded_scale: 0.5,
        }
    }
}

impl Default for InvalidateConfig {
    fn default() -> Self {
        Self {
            stale_after: Duration::from_secs(60),
            priority_micro_lamports: 100_000,
            state_dir: crate::quote_state::DEFAULT_STATE_DIR.to_string(),
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
            invalidate: InvalidateConfig::default(),
            fair_value: FairValueConfig {
                // One bound has to cover legs whose cadences differ by orders
                // of magnitude: Pyth republishes every second or so, while the
                // Frankfurter fallback behind it is a once-a-working-day ECB
                // reference. The bound is therefore set by the *slowest* leg —
                // anything tighter would drop the fallback out of the
                // composition permanently, which is the tier that keeps the six
                // CEX-less exotics quoting at all.
                //
                // The cost is that a dead Pyth feed is not caught for 15 min on
                // its own; in practice the weekend flip is what this governs,
                // and Pyth ages from its `publish_time` (not from receipt), so
                // a frozen FX session does go stale here rather than reading as
                // perpetually fresh. TBD(analytics): split per leg, which is
                // the real fix and belongs to the analytics.
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
            assert!(w[1].expiry_offset_secs > w[0].expiry_offset_secs);
            assert!(w[1].expiry_offset_slots > w[0].expiry_offset_slots);
        }
    }

    /// The staleness bound sits between the two cadences it has to respect: it
    /// must exceed the reference heartbeat (or a healthy bot would keep killing
    /// its own fresh book) and stay far under the deepest ladder tier's
    /// wall-clock life (or a resting level could outlive the bound and expire on
    /// its own, which is the exposure this closes).
    ///
    /// The upper bound is pinned at **twice the heartbeat**, not at some fraction
    /// of the deepest tier: the tier is ~48 min, so a "far under" test against it
    /// would wave through several minutes — while the documented promise is that
    /// no level rests unattended for more than about a minute. Keying both ends
    /// to the heartbeat is what actually holds that promise.
    #[test]
    fn stale_bound_brackets_the_heartbeat() {
        let cfg = BotConfig::default();
        let heartbeat = cfg.strategy.ref_heartbeat;
        assert!(
            cfg.invalidate.stale_after > heartbeat,
            "a bound under the heartbeat would kill a healthy bot's own fresh book"
        );
        assert!(
            cfg.invalidate.stale_after <= heartbeat * 2,
            "a bound over 2x the heartbeat stops matching the documented \
             ~1-minute unattended window"
        );
        // And it must still leave the deepest tier's ~48 min life far away, so a
        // level can never outlive the bound and expire on its own instead.
        let deepest = DEFAULT_LADDER
            .iter()
            .map(|l| l.expiry_offset_secs)
            .max()
            .expect("ladder is non-empty");
        // `expiry_offset_secs` is wall-clock seconds, so the tier's life
        // needs no slot-pace conversion — the point of the datum model.
        let deepest_wall = Duration::from_secs(deepest as u64);
        assert!(cfg.invalidate.stale_after * 10 < deepest_wall);
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

    /// Every market names a Pyth FX feed, and each id is a distinct 32-byte
    /// hex string. A duplicated id would silently anchor two currencies on one
    /// cross — the copy-paste failure this roster is most exposed to.
    #[test]
    fn every_market_has_a_distinct_pyth_feed() {
        use std::collections::HashSet;
        let mut ids = HashSet::new();
        for m in MARKETS {
            assert_eq!(
                m.pyth_feed_id.len(),
                64,
                "{} pyth feed id is 32 bytes of hex",
                m.symbol
            );
            assert!(
                m.pyth_feed_id.chars().all(|c| c.is_ascii_hexdigit()),
                "{} pyth feed id is hex",
                m.symbol
            );
            assert!(
                ids.insert(m.pyth_feed_id),
                "{} reuses another market's pyth feed",
                m.symbol
            );
        }
    }

    /// The CEX tiers are opt-in per token and must not be invented: only EURC
    /// is listed on Coinbase or Kraken, and every entry that does exist has to
    /// name the market's own currency so a mis-pasted pair can't quote the
    /// wrong token.
    #[test]
    fn cex_basis_tiers_are_only_claimed_where_the_token_is_listed() {
        for m in MARKETS {
            if m.symbol == "EURC" {
                assert_eq!(m.coinbase_product, Some("EURC-USDC"));
                assert_eq!(m.kraken_pair, Some("EURCUSD"));
            } else {
                assert!(m.coinbase_product.is_none(), "{} on Coinbase?", m.symbol);
                assert!(m.kraken_pair.is_none(), "{} on Kraken?", m.symbol);
            }
            for pair in [m.coinbase_product, m.kraken_pair].into_iter().flatten() {
                assert!(
                    pair.starts_with(m.symbol),
                    "{} names the pair {pair}",
                    m.symbol
                );
            }
        }
    }
}
