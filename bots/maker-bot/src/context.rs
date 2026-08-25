//! Runtime context — the chain handle, the leader identity, the discovered
//! market, and the mutable bookkeeping the tick loop carries between ticks.
//!
//! State is kept deliberately thin and chain-derived: the live vault snapshot
//! is re-read every tick (it is the source of truth for inventory and fills),
//! and the bot only remembers what it can't recover from a single read — the
//! last reference it stamped, the skew it applied, when it last fired each
//! path, and which profile shape it believes is armed.
//!
//! One fact outlives the process: the wall-clock time of the last live reference
//! stamp, which the chain cannot supply (it records the `quote_slot`, and slots
//! stop ticking during a halt). That one is persisted per market — see
//! [`crate::quote_state`] — so a restart can tell a fresh resting book from one
//! that needs killing.

use crate::config::MarketConfig;
use crate::model::ladder::Side;
use crate::quote_state::QuoteState;
use crate::telemetry::Telemetry;
use dropset_fair_value::{FairValueConfig, FairValueEngine};
use solana_client::rpc_client::RpcClient;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

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
    /// Whether that reference price still lets the matching engine visit this
    /// vault (the program's own `has_valid_reference_price` gate). `false` means
    /// the book is already dark — never stamped, or killed by the stale-quote
    /// invalidation — so there is nothing for `model::invalidate` to kill.
    pub reference_valid: bool,
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
    /// The leader / quote-authority signer, shared by every market's context
    /// rather than copied into each — one long-lived copy of the 32-byte
    /// secret regardless of roster size.
    ///
    /// `Keypair` is deliberately not `Clone` upstream, so `.clone()` here can
    /// only clone the handle: the type closes the *accidental* path back to a
    /// copy per market, not the deliberate one (`insecure_clone` stays
    /// reachable through `Deref`). The narrow signer interface that would
    /// close that is specified in `docs/key-custody.md` §3.1.
    ///
    /// `Deref` is also what leaves the use sites unchanged — `&ctx.leader`
    /// coerces to the `&Keypair` the `chain` helpers take, and
    /// `ctx.leader.pubkey()` resolves — so the secret half stays reachable
    /// only from those signing calls, the property the recurring key-custody
    /// audit re-derives.
    pub leader: Arc<Keypair>,
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
    /// Whether the one-shot startup basis sanity check has run for this market.
    ///
    /// Keyed on the first tick where a basis is actually *observable* (both the
    /// FX and crypto legs live), not on the first tick: the feed sources warm
    /// asynchronously on a background runtime, so the earliest ticks routinely
    /// have no basis to check and would spend the one shot on nothing.
    pub basis_checked: bool,

    /// The last leg-health line reported for this market, so the tick loop logs
    /// a **transition** rather than the same line every five seconds.
    ///
    /// The consensus filter produces per-tick signals — a refused observation,
    /// a leg whose sources disagree, a basis carried past its age — and a signal
    /// nothing reads is a signal that does not exist. Deduping here is what
    /// makes logging them affordable on a per-tick loop; the richer operator
    /// surface is the separate telemetry effort's, not this field's.
    pub last_leg_health: Option<String>,

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
    /// When the hot path last fired. Seeded at construction from the persisted
    /// record (see [`Context::new`]) rather than from process start, so the
    /// staleness check that ages off it measures the resting book's real age
    /// across a restart.
    pub last_set_at: Instant,
    /// When the cold path last fired.
    pub last_profile_at: Instant,
    /// This market's persisted last-live-stamp record — the one piece of state
    /// that survives a restart, because it is the one the chain cannot supply
    /// (see [`crate::quote_state`]).
    pub quote_state: QuoteState,
    /// Whether the bot has already killed this market's book for staleness in
    /// this run and not yet re-armed it. Keeps the running-path invalidation to
    /// one instruction per stale episode instead of one per cycle; cleared by the
    /// next live reference stamp.
    pub reference_invalidated: bool,
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
    /// Where this market's per-tick operational sample is published. Cloned
    /// per market from one channel, and [`Telemetry::disabled`] when no
    /// telemetry database is configured — so the tick loop emits
    /// unconditionally and never branches on whether anyone is listening.
    pub telemetry: Telemetry,
}

impl Context {
    /// Build a context around a discovered market, starting the cadence clocks
    /// in the past so the first tick can establish the reference immediately.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: RpcClient,
        leader: Arc<Keypair>,
        vault_idx: u32,
        market: MarketAddrs,
        cfg: MarketConfig,
        fair_value: FairValueConfig,
        quote_state: QuoteState,
        telemetry: Telemetry,
    ) -> Self {
        let now = Instant::now();
        // Start the hot-path clock from the persisted record, not from process
        // start. The running-path staleness check ages off `last_set_at`, so a
        // placeholder here would re-credit a restarted bot a full staleness
        // bound the resting book hasn't earned: a record already 55 s old (still
        // inside the bound, so the startup pass rightly declines to kill) would
        // then not be killed until 60 s *after startup* — ~115 s of unattended
        // matchable book, twice the bound this is supposed to hold. Seeding also
        // covers the case where the startup pass's vault read fails and never
        // gets to consult the record at all.
        //
        // Reading the record here rather than reusing the startup pass's read
        // keeps the two questions separate: that pass needs the `Option` (an
        // *unknown* age is what it treats as stale), while this only needs a
        // clock origin, so an unknown age correctly falls back to `now`.
        // `checked_sub` guards a freshly-booted host whose monotonic clock is
        // younger than the record's age.
        let last_set_at = quote_state
            .age(SystemTime::now())
            .and_then(|age| now.checked_sub(age))
            .unwrap_or(now);
        // The calibration is shared across markets but the pinned basis is a
        // property of *this* market's source coverage, so it is layered on here.
        // Routed through the crate helper rather than spelled out, because this
        // is not the only site that builds an engine — the dry-run path does too
        // — and a site that forgot the pin would quietly quote the market on the
        // no-basis-leg degrade path with no failing test behind it.
        let fair_value = fair_value.with_pinned_basis(cfg.pinned_basis);
        Self {
            quote_state,
            reference_invalidated: false,
            client,
            leader,
            vault_idx,
            market,
            cfg,
            engine: FairValueEngine::new(fair_value),
            last_compose: None,
            basis_checked: false,
            last_leg_health: None,
            launch_tvl_usd: None,
            fills_active: false,
            last_set_price: None,
            last_skew_bps: 0.0,
            last_set_at,
            last_profile_at: now,
            profile_kind: ProfileKind::Unknown,
            last_inventory: None,
            position: None,
            telemetry,
        }
    }
}
