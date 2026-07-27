//! The supervisor tick loop (§3 bot heartbeat).
//!
//! The demo runs many markets at once. A single process supervises them: each
//! cycle (5 s) it refreshes the **shared, batched** feed cache once — one
//! CoinGecko call prices every token, one Frankfurter call covers every
//! currency, CoinMarketCap is the on-failure secondary — then walks the
//! markets, composing each one's reference from the cache and firing **at most
//! one** instruction per market. The cold path (`set_liquidity_profile`) takes
//! precedence over the hot path (`set_reference_price`) when a reshape is due,
//! so a market never sends both in one cycle. A failed send is logged and that
//! market is skipped; the next cycle retries (no retry storms).
//!
//! Fill detection is driven by the `emit_cpi!` `FillEvent` subscription
//! (`fills` module, §3 production-fidelity path). One subscription covers every
//! market the shared leader quotes; the supervisor drains it each cycle and
//! routes each fill to its market by `event.market`, advancing that market's
//! fill-derived position. The per-market vault read reconciles the position
//! (catching a missed fill or external flow) and is the sole fill signal in the
//! fallback path — when no subscription is attached, a balance change the bot
//! didn't cause is taken as a fill. (The reference's price-time nonce is *not*
//! used — it bumps on every re-quote, so it can't tell a fill from a re-quote.)

use crate::chain;
use crate::config::{BotConfig, MarketConfig, USDC_COINGECKO_ID};
use crate::context::{Context, ProfileKind, VaultSnapshot};
use crate::fills::Fill;
use crate::model::fair_mid::{build_legs, FairValue};
use crate::model::inventory::Inventory;
use crate::model::killswitch::{self, Action, HaltReason};
use crate::model::ladder::{self, Side};
use crate::model::skew;
use crate::model::triggers::{self, RefTrigger};
use anyhow::Result;
use dropset_fair_value::{Legs, Reading};
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use std::collections::HashMap;
use std::hash::Hash;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast::{self, error::TryRecvError};

/// The live-sink receivers the supervisor drains each cycle, one per price
/// tier. Each is the read end of a `feeds` forward (live) sink fed by that
/// tier's source running on the background runtime (`main.rs`); the source owns
/// its own poll cadence and error backoff, so the supervisor only reads the
/// tail. CoinMarketCap is `None` when the secondary tier isn't wired up (no
/// `CMC_API_KEY`).
pub struct FeedReceivers {
    pub coingecko: broadcast::Receiver<HashMap<String, f64>>,
    pub coinmarketcap: Option<broadcast::Receiver<HashMap<u32, f64>>>,
    pub frankfurter: broadcast::Receiver<HashMap<String, f64>>,
}

/// Drain every reading queued on `rx` into `cache`, stamping `now` as the read
/// time the engine's freshness rules age from. Stops on an empty or closed
/// channel (a closed source's last reading is left to age out); a lag — the
/// source outran a slow cycle — skips to the retained latest and keeps
/// draining, since the freshest reading is the one the cache wants.
fn drain_into<K: Eq + Hash + Clone>(
    rx: &mut broadcast::Receiver<HashMap<K, f64>>,
    cache: &mut HashMap<K, (f64, Instant)>,
    now: Instant,
) {
    loop {
        match rx.try_recv() {
            Ok(readings) => {
                for (k, v) in readings {
                    cache.insert(k, (v, now));
                }
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(_)) => continue,
        }
    }
}

/// The shared, batched feed cache. Each tier's source polls the whole roster in
/// one batched call and forwards a keyed reading map onto a live sink; a cycle
/// drains those maps into the per-tier caches below, and `legs()` picks the
/// freshest live leg per market. CoinMarketCap is the crypto-basis fallback:
/// `legs()` uses it only when the CoinGecko reading for a market is absent.
struct FeedHub {
    /// `coingecko_id → (usd, when read)`.
    cg: HashMap<String, (f64, Instant)>,
    /// `cmc numeric id → (usd, when read)`.
    cmc: HashMap<u32, (f64, Instant)>,
    /// `currency → (usd per unit, when read)`.
    fx: HashMap<String, (f64, Instant)>,
}

impl FeedHub {
    fn new() -> Self {
        Self {
            cg: HashMap::new(),
            cmc: HashMap::new(),
            fx: HashMap::new(),
        }
    }

    /// Drain each tier's live-sink receiver into the cache, stamping `now`. The
    /// CoinMarketCap tier is drained only when it's wired up this run.
    fn drain(&mut self, now: Instant, rx: &mut FeedReceivers) {
        drain_into(&mut rx.coingecko, &mut self.cg, now);
        if let Some(cmc) = rx.coinmarketcap.as_mut() {
            drain_into(cmc, &mut self.cmc, now);
        }
        drain_into(&mut rx.frankfurter, &mut self.fx, now);
    }

    /// This market's cached readings, aged to `now`, mapped onto the engine's
    /// [`Legs`] (§1): Frankfurter USD/`<ccy>` is the FX anchor, CoinGecko / CMC
    /// token-USD is the demoted crypto basis leg, CoinGecko `usd-coin` is the
    /// USDC/USD common-mode leg, and the market's static peg is the last resort.
    fn legs(&self, now: Instant, market: &MarketConfig) -> Legs {
        let aged =
            |o: Option<&(f64, Instant)>| o.map(|(v, t)| Reading::new(*v, now.duration_since(*t)));
        // FX anchor: the exogenous fiat cross (USD per the market's fiat).
        let fx = aged(self.fx.get(market.currency));
        // Crypto basis leg (demoted from the old primary): CoinGecko token-USD,
        // falling back to CoinMarketCap.
        let cg = aged(self.cg.get(market.coingecko_id));
        let cmc = market
            .coinmarketcap_id
            .and_then(|id| self.cmc.get(&id))
            .map(|(v, t)| Reading::new(*v, now.duration_since(*t)));
        let crypto_usdc = cg.or(cmc);
        // USDC/USD common-mode leg, shared across every market.
        let usdc_usd = aged(self.cg.get(USDC_COINGECKO_ID));
        build_legs(fx, crypto_usdc, usdc_usd, market.static_usd)
    }
}

/// Whether the Unix timestamp `secs` falls in the FX-closed weekend window.
/// Interbank FX and CME 6E are shut Fri ~17:00 → Sun ~17:00 ET (§1 fm2);
/// approximated here in UTC as Fri 21:00 → Sun 22:00 (≈ 17:00 ET, ignoring
/// DST). The exact session thresholds are TBD(survey). Inside this window a
/// missing FX anchor is the normal crypto-only regime, not a fault.
fn weekend_from_unix(secs: u64) -> bool {
    let days = secs / 86_400; // whole days since 1970-01-01 (a Thursday)
    let hour = (secs % 86_400) / 3_600; // hour of the UTC day
    let dow = (days + 4) % 7; // 0 = Sun … 6 = Sat (epoch day was Thursday = 4)
    match dow {
        5 => hour >= 21, // Friday, after the interbank close
        6 => true,       // all of Saturday
        0 => hour < 22,  // Sunday, until the CME reopen
        _ => false,
    }
}

/// [`weekend_from_unix`] for the wall clock. A clock before the Unix epoch
/// (unreachable in practice) reads as a weekday.
fn is_weekend(now: SystemTime) -> bool {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    weekend_from_unix(secs)
}

/// Run the supervisor over every market until interrupted. Each loop iteration
/// is one cycle; a per-market error is logged and the others continue.
pub fn run_supervisor(
    mut feeds: FeedReceivers,
    cfg: BotConfig,
    mut markets: Vec<Context>,
    mut fills: Option<broadcast::Receiver<Fill>>,
) -> Result<()> {
    let fills_active = fills.is_some();
    for ctx in &mut markets {
        ctx.fills_active = fills_active;
    }
    println!(
        "maker-bot live: {} markets (tick {:?}, fills {})",
        markets.len(),
        cfg.tick,
        if fills_active { "on" } else { "off" }
    );

    let mut hub = FeedHub::new();
    loop {
        let now = Instant::now();
        // Drain each price tier's live sink into the cache. The sources on the
        // background runtime own the poll cadence and error backoff, so the
        // tick just reads whatever landed since last cycle.
        hub.drain(now, &mut feeds);

        // Drain the fill live sink and route each fill to its market.
        let (routed, disconnected) = drain_fills(fills.as_mut(), &markets);
        if disconnected {
            fills = None;
            for ctx in &mut markets {
                ctx.fills_active = false;
            }
        }

        // The FX session is closed the same wall-clock window for every market.
        let weekend = is_weekend(SystemTime::now());
        for ctx in &mut markets {
            let legs = hub.legs(now, &ctx.cfg);
            let dt = ctx
                .last_compose
                .map_or(Duration::ZERO, |t| now.duration_since(t));
            ctx.last_compose = Some(now);
            let fair = ctx.engine.compose(legs, dt, weekend);
            let got_fill = routed.get(&ctx.market.market).copied();
            if let Err(e) = quote_market(ctx, &cfg, now, fair, got_fill) {
                eprintln!("[{}] tick error: {e}", ctx.cfg.symbol);
            }
        }
        std::thread::sleep(cfg.tick);
    }
}

/// Drain every attributed fill delivered since the last cycle and route it to
/// its market by `event.market`, keeping the chain-latest (highest
/// `nonce_after`) per market — channel-arrival order isn't guaranteed to be
/// slot order. Returns `market → (base_after, quote_after)` plus whether the
/// live sink closed (the `feeds` runner stopped — a bare subscription-thread
/// panic instead idles the stream seam), so the caller can revert every market
/// to the inventory-diff fallback.
///
/// Routing is by market alone: the bootstrap opens exactly one leader vault
/// (sector) per market, and the leader quotes only that sector, so a fill
/// against this leader on this market is unambiguously this vault's. A market
/// with more than one leader-owned sector would need `event.sector_idx`
/// disambiguation too — not a shape this localnet demo creates.
fn drain_fills(
    fills: Option<&mut broadcast::Receiver<Fill>>,
    markets: &[Context],
) -> (HashMap<Pubkey, (u64, u64)>, bool) {
    let mut best: HashMap<Pubkey, (u64, u64, u64)> = HashMap::new();
    let Some(rx) = fills else {
        return (HashMap::new(), false);
    };
    let symbol = |market: &Pubkey| {
        markets
            .iter()
            .find(|c| &c.market.market == market)
            .map_or("?", |c| c.cfg.symbol)
    };
    let mut disconnected = false;
    loop {
        match rx.try_recv() {
            Ok(fill) => {
                let e = &fill.event;
                let side = if e.side == 0 { "ask" } else { "bid" };
                println!(
                    "[{}][fill] {side} L{} {} base / {} quote @ {} (fee {} atoms, sig {})",
                    symbol(&e.market),
                    e.level_idx,
                    e.fill_base,
                    e.fill_quote,
                    e.fill_price,
                    e.taker_fee_atoms,
                    fill.signature
                );
                let entry = best.entry(e.market).or_insert((0, 0, 0));
                if e.nonce_after >= entry.0 {
                    *entry = (e.nonce_after, e.base_atoms_after, e.quote_atoms_after);
                }
            }
            Err(TryRecvError::Empty) => break,
            // The forward sink dropped fills the cycle didn't keep up with. The
            // reconcile keeps only the highest-`nonce_after` fill per market and
            // the sink drops to the latest, so the freshest position survives;
            // note the gap and keep draining the retained records.
            Err(TryRecvError::Lagged(n)) => {
                eprintln!("[fills] lagged {n} fills; reconciling to the latest per market");
                continue;
            }
            Err(TryRecvError::Closed) => {
                eprintln!(
                    "[fills] subscription channel closed; reverting to inventory-diff fallback"
                );
                disconnected = true;
                break;
            }
        }
    }
    let routed = best
        .into_iter()
        .map(|(m, (_, base, quote))| (m, (base, quote)))
        .collect();
    (routed, disconnected)
}

/// Quote one market for this cycle: read its vault, value inventory off the
/// composed reference, and fire at most one instruction.
fn quote_market(
    ctx: &mut Context,
    cfg: &BotConfig,
    now: Instant,
    fair: FairValue,
    got_fill: Option<(u64, u64)>,
) -> Result<()> {
    let vault = chain::read_vault(
        &ctx.client,
        &ctx.market.market,
        &ctx.leader.pubkey(),
        ctx.market.base_decimals,
        ctx.market.quote_decimals,
    )?;
    ctx.vault_idx = vault.sector_idx;

    // A fill the supervisor routed to this market is fresher than the vault
    // read taken above, so it leads the reconcile.
    if let Some(pos) = got_fill {
        ctx.position = Some(pos);
    }
    let (base_atoms, quote_atoms) = resolve_inventory(ctx, &vault, got_fill.is_some());

    if vault.frozen {
        println!(
            "[{}][halt] vault is frozen on-chain — idling",
            ctx.cfg.symbol
        );
        return Ok(());
    }

    let Some(mid) = fair.fair else {
        println!(
            "[{}][pause] {:?}: no usable feed, holding reference",
            ctx.cfg.symbol, fair.regime
        );
        return Ok(());
    };
    // Surface the live anchor / regime so an operator sees which leg is pricing
    // this market whenever the composition is running degraded (§1, §4).
    if fair.degraded() {
        println!(
            "[{}] quoting off {:?} ({:?}, degraded)",
            ctx.cfg.symbol, fair.anchor, fair.regime
        );
    }

    let inv = Inventory::from_atoms(
        base_atoms,
        quote_atoms,
        ctx.market.base_decimals,
        ctx.market.quote_decimals,
        mid,
    );
    // Baseline the drawdown floor against the first valued TVL of this run,
    // logging the adopted baseline so the operator can see which floor the run
    // is holding to (the floor is meaningless if the vault was read near-empty).
    let launch_tvl = match ctx.launch_tvl_usd {
        Some(tvl) => tvl,
        None => {
            let tvl = inv.total_usd();
            ctx.launch_tvl_usd = Some(tvl);
            println!(
                "[baseline] launch TVL ${tvl:.2} — drawdown floor at {:.0}%",
                cfg.kill.tvl_floor_frac * 100.0
            );
            tvl
        }
    };

    let degraded = fair.degraded();
    let action = killswitch::evaluate(&fair, &inv, &cfg.kill, degraded, launch_tvl);
    let skew_bps = skew::ref_skew_bps(&inv, &cfg.strategy);
    let reference = skew::apply_skew(mid, skew_bps);

    // Cold path first — at most one ix per cycle.
    match action {
        Action::Halt(reason) => {
            halt(ctx, cfg, reason)?;
            return Ok(());
        }
        Action::FreezeSide(side) => {
            if ctx.profile_kind != ProfileKind::FrozenSide(side) {
                freeze_side(ctx, cfg, side)?;
                return Ok(());
            }
        }
        Action::Reshape(accumulating) => {
            if ctx.profile_kind != ProfileKind::Reshaped(accumulating)
                || profile_heartbeat_due(ctx, cfg, now)
            {
                arm_reshape(ctx, cfg, accumulating, now)?;
                return Ok(());
            }
        }
        Action::Quote => {
            if standard_arm_due(ctx, cfg, now) {
                arm_standard(ctx, cfg, now)?;
                return Ok(());
            }
        }
    }

    // Hot path — refresh the reference when a trigger fires.
    let trig = RefTrigger {
        candidate: reference,
        last_set: ctx.last_set_price,
        since_last_set: now.duration_since(ctx.last_set_at),
        skew_bps,
        last_skew_bps: ctx.last_skew_bps,
    };
    if triggers::should_set_reference(&trig, &cfg.strategy) {
        let slot = chain::current_slot(&ctx.client)?;
        chain::set_reference_price(
            &ctx.client,
            &ctx.leader,
            &ctx.market.market,
            ctx.vault_idx,
            reference,
            ctx.market.base_decimals,
            ctx.market.quote_decimals,
            slot,
        )?;
        ctx.last_set_price = Some(reference);
        ctx.last_skew_bps = skew_bps;
        ctx.last_set_at = now;
        println!(
            "[{}][ref] set {reference:.8} (skew {skew_bps:+.1} bps, slot {slot})",
            ctx.cfg.symbol
        );
    }
    Ok(())
}

/// Resolve the inventory the policy values this cycle, in `(base, quote)`
/// atoms. With a subscription attached, the fill-derived position is
/// authoritative (seeded from the first vault read, advanced by routed fills,
/// reconciled against the per-cycle vault read). Without one, the vault read is
/// the only signal and a balance the bot didn't move is logged as a fill.
fn resolve_inventory(ctx: &mut Context, vault: &VaultSnapshot, drained: bool) -> (u64, u64) {
    let chain = (vault.base_atoms, vault.quote_atoms);

    if !ctx.fills_active {
        if let Some(prev) = ctx.last_inventory {
            if prev != chain {
                let db = chain.0 as i128 - prev.0 as i128;
                let dq = chain.1 as i128 - prev.1 as i128;
                println!(
                    "[{}][fill] inventory moved: base {db:+}, quote {dq:+} atoms",
                    ctx.cfg.symbol
                );
            }
        }
        ctx.last_inventory = Some(chain);
        return chain;
    }

    let (inventory, position, reconciled) = decide_position(ctx.position, chain, drained);
    if reconciled {
        let (pb, pq) = ctx.position.unwrap_or(chain);
        println!(
            "[{}][fills] reconciling to chain: position ({pb}, {pq}) vs vault ({}, {}) — missed fill or external flow",
            ctx.cfg.symbol, chain.0, chain.1
        );
    }
    ctx.position = Some(position);
    inventory
}

/// The fill-path inventory decision, factored out as a pure function over plain
/// values so it can be unit-tested without a live `Context`.
///
/// Returns `(inventory_to_value, position_to_store, reconciled)`:
/// - no position yet → seed it from the chain read;
/// - a fill landed this cycle → the position is fresher than the vault read
///   taken before the drain, so trust it (no reconcile);
/// - no fill this cycle but the position disagrees with the chain → a missed
///   fill or external deposit / withdraw, so the chain wins (`reconciled`);
/// - otherwise the position already matches the chain, so keep it.
fn decide_position(
    position: Option<(u64, u64)>,
    chain: (u64, u64),
    drained: bool,
) -> ((u64, u64), (u64, u64), bool) {
    match position {
        None => (chain, chain, false),
        Some(pos) if drained => (pos, pos, false),
        Some(pos) if pos != chain => (chain, chain, true),
        Some(pos) => (pos, pos, false),
    }
}

/// Whether the daily `SetLiquidityProfile` heartbeat is due — re-arm even an
/// unchanged shape this often so deep, rarely-filled levels don't expire dark.
fn profile_heartbeat_due(ctx: &Context, cfg: &BotConfig, now: Instant) -> bool {
    triggers::should_set_profile_heartbeat(
        now.duration_since(ctx.last_profile_at),
        cfg.strategy.profile_heartbeat,
    )
}

/// Whether the standard ladder needs re-arming this cycle — either it isn't the
/// armed shape (first cycle, or recovering from a halt/freeze/reshape) or the
/// daily heartbeat is due.
fn standard_arm_due(ctx: &Context, cfg: &BotConfig, now: Instant) -> bool {
    ctx.profile_kind != ProfileKind::Standard || profile_heartbeat_due(ctx, cfg, now)
}

/// Arm the full symmetric ladder.
fn arm_standard(ctx: &mut Context, cfg: &BotConfig, now: Instant) -> Result<()> {
    let profile = ladder::build_profile(&cfg.strategy.ladder);
    chain::set_liquidity_profile(
        &ctx.client,
        &ctx.leader,
        &ctx.market.market,
        ctx.vault_idx,
        ladder::checked_bytes(&profile)?,
    )?;
    ctx.profile_kind = ProfileKind::Standard;
    ctx.last_profile_at = now;
    println!("[{}][profile] armed standard ladder", ctx.cfg.symbol);
    Ok(())
}

/// Shrink the accumulating side so the heavy (rebuild) side dominates the book
/// and leans into offloading the heavy leg — the §4 row 1 reshape (imbalance
/// over 30%), a milder step than the freeze. The reference skew (applied every
/// cycle) supplies the price shift that invites rebalancing.
fn arm_reshape(ctx: &mut Context, cfg: &BotConfig, accumulating: Side, now: Instant) -> Result<()> {
    let mut profile = ladder::build_profile(&cfg.strategy.ladder);
    ladder::scale_side(
        &mut profile,
        accumulating,
        cfg.strategy.reshape_accumulating_scale,
    );
    chain::set_liquidity_profile(
        &ctx.client,
        &ctx.leader,
        &ctx.market.market,
        ctx.vault_idx,
        ladder::checked_bytes(&profile)?,
    )?;
    ctx.profile_kind = ProfileKind::Reshaped(accumulating);
    ctx.last_profile_at = now;
    let rebuild = match accumulating {
        Side::Bid => Side::Ask,
        Side::Ask => Side::Bid,
    };
    println!(
        "[{}][reshape] shrank {accumulating:?} side — grew {rebuild:?} side to rebalance",
        ctx.cfg.symbol
    );
    Ok(())
}

/// Zero the accumulating side so only the rebuild side quotes (§4).
fn freeze_side(ctx: &mut Context, cfg: &BotConfig, side: Side) -> Result<()> {
    let mut profile = ladder::build_profile(&cfg.strategy.ladder);
    ladder::zero_side(&mut profile, side);
    chain::set_liquidity_profile(
        &ctx.client,
        &ctx.leader,
        &ctx.market.market,
        ctx.vault_idx,
        ladder::checked_bytes(&profile)?,
    )?;
    ctx.profile_kind = ProfileKind::FrozenSide(side);
    ctx.last_profile_at = Instant::now();
    println!(
        "[{}][freeze] zeroed {side:?} side — only the rebuild side quotes",
        ctx.cfg.symbol
    );
    Ok(())
}

/// Stop quoting and alert. The bot zeroes both sides (leader-authorized) and
/// leaves the irreversible, admin-only `FreezeVault` to a human.
fn halt(ctx: &mut Context, cfg: &BotConfig, reason: HaltReason) -> Result<()> {
    eprintln!(
        "[{}][ALERT] kill switch: {reason:?} — halting quotes for review",
        ctx.cfg.symbol
    );
    if ctx.profile_kind != ProfileKind::Halted {
        let mut profile = ladder::build_profile(&cfg.strategy.ladder);
        ladder::zero_side(&mut profile, Side::Bid);
        ladder::zero_side(&mut profile, Side::Ask);
        chain::set_liquidity_profile(
            &ctx.client,
            &ctx.leader,
            &ctx.market.market,
            ctx.vault_idx,
            ladder::checked_bytes(&profile)?,
        )?;
        ctx.profile_kind = ProfileKind::Halted;
        ctx.last_profile_at = Instant::now();
        println!(
            "[{}][halt] zeroed both sides; existing levels expire on their own",
            ctx.cfg.symbol
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_read_seeds_the_position() {
        let (inv, pos, reconciled) = decide_position(None, (100, 200), false);
        assert_eq!(inv, (100, 200));
        assert_eq!(pos, (100, 200));
        assert!(!reconciled);
    }

    #[test]
    fn a_fill_this_cycle_leads_the_pre_drain_vault_read() {
        let (inv, pos, reconciled) = decide_position(Some((90, 210)), (100, 200), true);
        assert_eq!(inv, (90, 210));
        assert_eq!(pos, (90, 210));
        assert!(!reconciled);
    }

    #[test]
    fn a_quiet_cycle_matching_chain_keeps_the_position() {
        let (inv, pos, reconciled) = decide_position(Some((100, 200)), (100, 200), false);
        assert_eq!(inv, (100, 200));
        assert_eq!(pos, (100, 200));
        assert!(!reconciled);
    }

    #[test]
    fn divergence_without_a_fill_reconciles_to_chain() {
        let (inv, pos, reconciled) = decide_position(Some((90, 210)), (100, 200), false);
        assert_eq!(inv, (100, 200));
        assert_eq!(pos, (100, 200));
        assert!(reconciled);
    }

    #[test]
    fn weekend_window_brackets_the_fx_session_close() {
        // Anchored to known UTC instants in Jan 2021: the 1st was a Friday.
        const FRI_00: u64 = 1_609_459_200; // 2021-01-01 00:00 UTC (Friday)
        let h = |base: u64, hour: u64| base + hour * 3_600;
        let d = |base: u64, days: u64| base + days * 86_400;

        // Friday: open through the day, closed from 21:00 UTC.
        assert!(!weekend_from_unix(h(FRI_00, 12)));
        assert!(weekend_from_unix(h(FRI_00, 21)));
        // Saturday: closed all day.
        assert!(weekend_from_unix(h(d(FRI_00, 1), 3)));
        // Sunday: closed until the 22:00 UTC reopen, then open.
        assert!(weekend_from_unix(h(d(FRI_00, 2), 21)));
        assert!(!weekend_from_unix(h(d(FRI_00, 2), 23)));
        // Monday: open.
        assert!(!weekend_from_unix(h(d(FRI_00, 3), 12)));
    }

    #[test]
    fn drain_into_caches_the_latest_reading() {
        let (tx, mut rx) = broadcast::channel::<HashMap<String, f64>>(8);
        tx.send(HashMap::from([("euro-coin".to_string(), 1.10)]))
            .unwrap();
        tx.send(HashMap::from([("euro-coin".to_string(), 1.14)]))
            .unwrap();
        let mut cache: HashMap<String, (f64, Instant)> = HashMap::new();
        let now = Instant::now();
        drain_into(&mut rx, &mut cache, now);
        // Both readings drained; the later one wins the cache slot.
        assert_eq!(cache["euro-coin"].0, 1.14);
        // The channel is now empty — a second drain is a no-op.
        drain_into(&mut rx, &mut cache, now);
        assert_eq!(cache["euro-coin"].0, 1.14);
    }

    #[test]
    fn drain_into_skips_a_lag_to_the_retained_latest() {
        // Capacity 2, three readings before the drain: the receiver lags past
        // the dropped first one and still caches the newest.
        let (tx, mut rx) = broadcast::channel::<HashMap<String, f64>>(2);
        for px in [1.10, 1.12, 1.14] {
            tx.send(HashMap::from([("euro-coin".to_string(), px)]))
                .unwrap();
        }
        let mut cache: HashMap<String, (f64, Instant)> = HashMap::new();
        drain_into(&mut rx, &mut cache, Instant::now());
        assert_eq!(cache["euro-coin"].0, 1.14);
    }
}
