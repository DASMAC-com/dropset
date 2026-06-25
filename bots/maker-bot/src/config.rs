//! Bot configuration — the knobs `docs/market-making-mvp.md` pins down.
//!
//! Defaults encode the MVP spec verbatim: the CADC/USDC sources and poll
//! cadences (§1), the 50/100/200/500 bps ladder (§2), the `SetReferencePrice`
//! / `SetLiquidityProfile` triggers (§3), the linear inventory skew (§2), and
//! the inventory / peg / staleness kill-switch bounds (§4). Secrets (the Oanda
//! API key) come from the environment, never a committed field — the same
//! convention the Linear tooling uses.

use std::time::Duration;

/// Default localnet RPC endpoint (the `solana-test-validator` the TUI spawns).
pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:8899";

/// Derive the PubSub websocket endpoint from an RPC URL, matching the Agave
/// convention: swap the scheme (`http`→`ws`, `https`→`wss`) and use the RPC
/// port + 1 (the validator serves logs/account subscriptions there, so
/// `8899` → `8900`). Returns the input unchanged for an unrecognized scheme
/// (assume it is already a ws endpoint) or a non-numeric port.
pub fn ws_url_from_rpc(rpc_url: &str) -> String {
    let (scheme, rest) = if let Some(rest) = rpc_url.strip_prefix("https://") {
        ("wss://", rest)
    } else if let Some(rest) = rpc_url.strip_prefix("http://") {
        ("ws://", rest)
    } else {
        return rpc_url.to_string();
    };
    // PubSub lives at the root, so drop any path and bump the port.
    let authority = rest.split('/').next().unwrap_or(rest);
    let ws_authority = match authority.rsplit_once(':') {
        Some((host, port)) => match port.parse::<u16>() {
            Ok(port) => format!("{host}:{}", port + 1),
            Err(_) => authority.to_string(),
        },
        None => authority.to_string(),
    };
    format!("{scheme}{ws_authority}")
}

/// The vault the bootstrap opens first; the bot quotes this sector.
pub const DEFAULT_VAULT_IDX: u32 = 0;

/// The leader / quote-authority role key the ENG-515 bootstrap seeds the mock
/// vault with (`tui/src/market.rs` → `MOCK_CADC_USDC`). The bot signs the
/// vault-gated hot/cold path with it.
pub const DEFAULT_LEADER_KEY: &str = "keys/EEEE.json";

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

/// Price feeds and their poll cadences (§1). Identifiers, not secrets — the
/// Oanda API key is read from `OANDA_API_KEY` at run time.
#[derive(Clone, Debug)]
pub struct FeedConfig {
    /// CoinGecko coin id for CADC (primary CADC/USD source).
    pub coingecko_id: String,
    /// CoinGecko poll interval (10 s, under the 30 req/min free-tier ceiling).
    pub coingecko_poll: Duration,
    /// Oanda instrument for the FX sanity feed; inverted to CAD/USD.
    pub oanda_instrument: String,
    /// Oanda poll interval (15 s, M1 candles).
    pub oanda_poll: Duration,
    /// Oanda Practice REST base URL.
    pub oanda_base_url: String,
    /// Aerodrome (Base) CADC/USDC pool, via GeckoTerminal — a second CADC
    /// market-price source. `None` (the default) leaves `fair_mid` on the
    /// CoinGecko-only degraded path; your note flagged this feed as needing
    /// live testing before it feeds quoting.
    pub aerodrome: Option<AerodromeConfig>,
}

/// GeckoTerminal pool descriptor for the Aerodrome CADC/USDC source.
#[derive(Clone, Debug)]
pub struct AerodromeConfig {
    /// GeckoTerminal network slug (Base is `base`).
    pub network: String,
    /// Pool address on that network.
    pub pool: String,
    /// Poll interval (10 s via GeckoTerminal).
    pub poll: Duration,
}

/// Quoting strategy parameters (§2–§3).
#[derive(Clone, Debug)]
pub struct StrategyConfig {
    /// The quote ladder.
    pub ladder: Vec<LadderLevel>,
    /// Linear inventory skew: shift the reference by this many bps per $10 of
    /// signed inventory deviation (§2 override of the formal A-S skew).
    pub skew_bps_per_10usd: f64,
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
}

/// Inventory / peg / staleness kill-switch bounds (§1, §4).
#[derive(Clone, Copy, Debug)]
pub struct KillSwitchConfig {
    /// Per-side imbalance (% off the 50/50 launch split) that triggers a cold
    /// reshape (§4 row 1, §3 cold-path trigger).
    pub imbalance_reshape_pct: f64,
    /// Imbalance that freezes the heavy side (§4 row 2).
    pub imbalance_freeze_side_pct: f64,
    /// Imbalance that halts the whole vault for review (§4 row 3).
    pub imbalance_halt_pct: f64,
    /// Lower / upper CADC-peg bound vs FX spot; outside → halt (§1, §4).
    pub peg_low: f64,
    pub peg_high: f64,
    /// A feed older than this is stale → run degraded (§1, §4).
    pub feed_stale: Duration,
    /// CADC sources disagreeing by more than this (bps) → pause reference
    /// updates (§1, §4).
    pub cadc_disagree_bps: f64,
    /// Launch TVL (USD) and the floor that halts the vault for post-mortem
    /// (§4 last row).
    pub launch_tvl_usd: f64,
    pub tvl_halt_usd: f64,
}

/// The full bot configuration.
#[derive(Clone, Debug)]
pub struct BotConfig {
    /// RPC endpoint.
    pub rpc_url: String,
    /// PubSub websocket endpoint for the fill-event subscription. `None`
    /// derives it from `rpc_url` via [`ws_url_from_rpc`].
    pub ws_url: Option<String>,
    /// Vault sector the bot quotes.
    pub vault_idx: u32,
    /// Bot tick interval — the §3 5-second heartbeat.
    pub tick: Duration,
    pub feeds: FeedConfig,
    pub strategy: StrategyConfig,
    pub kill: KillSwitchConfig,
}

impl Default for FeedConfig {
    fn default() -> Self {
        Self {
            coingecko_id: "cad-coin".to_string(),
            coingecko_poll: Duration::from_secs(10),
            oanda_instrument: "USD_CAD".to_string(),
            oanda_poll: Duration::from_secs(15),
            oanda_base_url: "https://api-fxpractice.oanda.com".to_string(),
            aerodrome: None,
        }
    }
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            ladder: DEFAULT_LADDER.to_vec(),
            skew_bps_per_10usd: 5.0,
            skew_cap_bps: 20.0,
            ref_drift_bps: 10.0,
            ref_heartbeat: Duration::from_secs(30),
            ref_skew_change_bps: 2.0,
            profile_heartbeat: Duration::from_secs(24 * 3600),
        }
    }
}

impl Default for KillSwitchConfig {
    fn default() -> Self {
        Self {
            imbalance_reshape_pct: 30.0,
            imbalance_freeze_side_pct: 50.0,
            imbalance_halt_pct: 80.0,
            peg_low: 0.97,
            peg_high: 1.03,
            feed_stale: Duration::from_secs(5 * 60),
            cadc_disagree_bps: 50.0,
            launch_tvl_usd: 100.0,
            tvl_halt_usd: 80.0,
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

    /// The websocket endpoint swaps the scheme and uses the RPC port + 1.
    #[test]
    fn ws_url_follows_the_agave_convention() {
        assert_eq!(ws_url_from_rpc(DEFAULT_RPC_URL), "ws://127.0.0.1:8900");
        assert_eq!(
            ws_url_from_rpc("https://api.example.com:443/rpc"),
            "wss://api.example.com:444"
        );
        // Unrecognized scheme is assumed to already be a ws endpoint.
        assert_eq!(ws_url_from_rpc("ws://host:9000"), "ws://host:9000");
    }
}
