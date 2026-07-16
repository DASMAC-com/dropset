//! Runtime context — the chain handle, the leader identity, the discovered
//! market, and the mutable bookkeeping the tick loop carries between ticks.
//!
//! State is kept deliberately thin and chain-derived: the live vault snapshot
//! is re-read every tick (it is the source of truth for inventory and fills),
//! and the bot only remembers what it can't recover from a single read — the
//! last reference it stamped, the skew it applied, when it last fired each
//! path, and which profile shape it believes is armed.

use crate::config::MarketConfig;
use crate::model::ladder::Side;
use dropset_fair_value::{FairValueConfig, FairValueEngine};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use std::time::Instant;

/// The discovered market and its token metadata — everything the bot needs to
/// address the vault and value its inventory.
#[derive(Clone, Debug)]
pub struct MarketAddrs {
    pub market: Pubkey,
    pub base_mint: Pubkey,
    pub quote_mint: Pubkey,
    pub base_treasury: Pubkey,
    pub quote_treasury: Pubkey,
    pub base_decimals: u8,
    pub quote_decimals: u8,
}

/// A live snapshot of the bot's vault, re-read each tick.
#[derive(Clone, Copy, Debug)]
pub struct VaultSnapshot {
    /// The vault's sector index — addresses it in the quoting instructions.
    pub sector_idx: u32,
    pub base_atoms: u64,
    pub quote_atoms: u64,
    /// The reference price currently stamped on-chain, as a float.
    pub reference_price: f64,
    pub frozen: bool,
}

/// Which ladder shape the bot believes is armed on-chain. Tracked so the cold
/// path only re-issues when the shape actually changes (avoiding churn).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileKind {
    /// Not yet established this run.
    Unknown,
    /// The full symmetric ladder.
    Standard,
    /// The accumulating side shrunk so the heavy side dominates (a > 30%
    /// reshape); carries the *accumulating* side that was scaled down.
    Reshaped(Side),
    /// One side zeroed (a freeze-side reshape).
    FrozenSide(Side),
    /// Both sides zeroed (a halt).
    Halted,
}

/// One market's runtime context. The supervisor holds one per market — they
/// share a leader and a single fill subscription, but each tracks its own
/// vault, armed profile, and inventory belief.
pub struct Context {
    pub client: RpcClient,
    pub leader: Keypair,
    pub vault_idx: u32,
    pub market: MarketAddrs,
    /// The market's feed identity (CoinGecko / CoinMarketCap ids, the FX
    /// currency, the static peg) — what the engine needs to price this token.
    pub cfg: MarketConfig,
    /// This market's fair-value engine — `fair = fx × basis` plus the stateful
    /// basis EMA (§1). One per market: each carries its own basis history.
    pub engine: FairValueEngine,
    /// When the engine last composed for this market, for the basis-EMA decay.
    /// `None` until the first tick.
    pub last_compose: Option<Instant>,

    /// The vault's TVL (USD) the first time this run valued it — the baseline
    /// the §4 drawdown floor is measured against. Seeded on the first tick that
    /// has a usable mid; `None` until then. A restart re-baselines to the
    /// current TVL, which is fine for the short, attended demo run.
    pub launch_tvl_usd: Option<f64>,
    /// Whether a fill subscription is feeding this market (the supervisor sets
    /// it for every market when the subscription is live). Drives the
    /// fill-derived inventory path vs the inventory-diff fallback.
    pub fills_active: bool,
    /// Last reference price actually stamped, if any.
    pub last_set_price: Option<f64>,
    /// Inventory skew (bps) applied at the last stamp.
    pub last_skew_bps: f64,
    /// When the hot / cold paths last fired.
    pub last_set_at: Instant,
    pub last_profile_at: Instant,
    /// The profile shape the bot believes is armed.
    pub profile_kind: ProfileKind,
    /// Inventory `(base_atoms, quote_atoms)` at the previous tick — used by
    /// the fallback fill detection (a change the bot didn't cause is a fill)
    /// only when the event subscription is absent.
    pub last_inventory: Option<(u64, u64)>,
    /// Fill-derived inventory `(base_atoms, quote_atoms)` — the authoritative
    /// `*_after` balances off the chain-latest `FillEvent` the supervisor
    /// routed to this market, reconciled against the per-tick vault read.
    /// `None` until the first fill (or seeded from the first vault read).
    pub position: Option<(u64, u64)>,
}

impl Context {
    /// Build a context around a discovered market, starting the cadence clocks
    /// in the past so the first tick can establish the reference immediately.
    pub fn new(
        client: RpcClient,
        leader: Keypair,
        vault_idx: u32,
        market: MarketAddrs,
        cfg: MarketConfig,
        fair_value: FairValueConfig,
    ) -> Self {
        let now = Instant::now();
        Self {
            client,
            leader,
            vault_idx,
            market,
            cfg,
            engine: FairValueEngine::new(fair_value),
            last_compose: None,
            launch_tvl_usd: None,
            fills_active: false,
            last_set_price: None,
            last_skew_bps: 0.0,
            last_set_at: now,
            last_profile_at: now,
            profile_kind: ProfileKind::Unknown,
            last_inventory: None,
            position: None,
        }
    }
}
