//! Runtime context — the chain handle, the leader identity, the discovered
//! market, and the mutable bookkeeping the tick loop carries between ticks.
//!
//! State is kept deliberately thin and chain-derived: the live vault snapshot
//! is re-read every tick (it is the source of truth for inventory and fills),
//! and the bot only remembers what it can't recover from a single read — the
//! last reference it stamped, the skew it applied, when it last fired each
//! path, and which profile shape it believes is armed.

use crate::fills::Fill;
use crate::model::ladder::Side;
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use std::sync::mpsc::Receiver;
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

/// The bot's runtime context.
pub struct Context {
    pub client: RpcClient,
    pub leader: Keypair,
    pub vault_idx: u32,
    pub market: MarketAddrs,

    /// The vault's TVL (USD) the first time this run valued it — the baseline
    /// the §4 drawdown floor is measured against. Seeded on the first tick that
    /// has a usable mid; `None` until then. A restart re-baselines to the
    /// current TVL, which is fine for the short, attended demo run.
    pub launch_tvl_usd: Option<f64>,
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
    /// `*_after` balances off the chain-latest `FillEvent` drained this run,
    /// reconciled against the per-tick vault read. `None` until the first fill
    /// (or seeded from the first vault read).
    pub position: Option<(u64, u64)>,
    /// Attributed-fill channel from the subscription thread (the primary fill
    /// signal). `None` when fills aren't subscribed (`--dry-run`, no ws).
    pub fills: Option<Receiver<Fill>>,
}

impl Context {
    /// Build a context around a discovered market, starting the cadence clocks
    /// in the past so the first tick can establish the reference immediately.
    pub fn new(client: RpcClient, leader: Keypair, vault_idx: u32, market: MarketAddrs) -> Self {
        let now = Instant::now();
        Self {
            client,
            leader,
            vault_idx,
            market,
            launch_tvl_usd: None,
            last_set_price: None,
            last_skew_bps: 0.0,
            last_set_at: now,
            last_profile_at: now,
            profile_kind: ProfileKind::Unknown,
            last_inventory: None,
            position: None,
            fills: None,
        }
    }

    /// Attach the attributed-fill channel from the subscription thread.
    pub fn with_fills(mut self, fills: Receiver<Fill>) -> Self {
        self.fills = Some(fills);
        self
    }
}
