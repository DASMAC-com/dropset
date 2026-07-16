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
use crate::config::{BotConfig, FeedConfig, MarketConfig, USDC_COINGECKO_ID};
use crate::context::{Context, ProfileKind, VaultSnapshot};
use crate::fills::Fill;
use crate::model::fair_mid::{build_legs, FairValue};
use crate::model::feeds::Feeds;
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
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Whether a feed tier is due to poll again given its last poll and interval.
fn due(last: Option<Instant>, now: Instant, interval: Duration) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= interval)
}

/// The shared, batched feed cache. One refresh cycle polls CoinGecko for every
/// market's token at once and Frankfurter for every currency at once, so the
/// whole roster costs one request per tier rather than one per market.
/// CoinMarketCap is the on-failure secondary: polled only when the latest
/// CoinGecko poll came back empty, and on a slower spacing to respect the
/// free-tier quota.
struct FeedHub {
    /// `coingecko_id → (usd, when read)`.
    cg: HashMap<String, (f64, Instant)>,
    /// `cmc numeric id → (usd, when read)`.
    cmc: HashMap<u32, (f64, Instant)>,
    /// `currency → (usd per unit, when read)`.
    fx: HashMap<String, (f64, Instant)>,
    cg_last_poll: Option<Instant>,
    cmc_last_poll: Option<Instant>,
    fx_last_poll: Option<Instant>,
    /// Whether the most recent CoinGecko poll produced at least one price — the
    /// signal that gates the CoinMarketCap fallback.
    cg_ok: bool,
    /// Whether the current CoinGecko-down state has already been logged, so a
    /// persistent failure (e.g. CoinGecko unreachable on localnet) reports once
    /// and then stays quiet until it recovers — rather than one line per tick.
    cg_logged_down: bool,
    /// Current CoinGecko poll interval after on-failure backoff. `None` uses the
    /// configured base; a failure doubles it (capped) so a rate-limited feed is
    /// retried ever less often instead of hammered, and the backoff resets to
    /// the base once a poll succeeds.
    cg_backoff: Option<Duration>,
}

/// Upper bound on the CoinGecko backoff interval — a rate-limited feed is
/// retried at most this rarely before a success resets it.
const CG_BACKOFF_CAP: Duration = Duration::from_secs(300);

impl FeedHub {
    fn new() -> Self {
        Self {
            cg: HashMap::new(),
            cmc: HashMap::new(),
            fx: HashMap::new(),
            cg_last_poll: None,
            cmc_last_poll: None,
            fx_last_poll: None,
            cg_ok: false,
            cg_logged_down: false,
            cg_backoff: None,
        }
    }

    /// Grow the CoinGecko backoff interval after a failed / empty poll: double
    /// the current interval (starting from `base`), capped at [`CG_BACKOFF_CAP`],
    /// so a rate-limited or unreachable feed is retried ever less often. A
    /// successful poll resets it back to `None` (the configured base).
    fn grow_cg_backoff(&mut self, base: Duration) {
        let current = self.cg_backoff.unwrap_or(base);
        self.cg_backoff = Some(current.saturating_mul(2).min(CG_BACKOFF_CAP).max(base));
    }

    /// Refresh whichever tiers are due, batched across the whole roster.
    fn refresh(
        &mut self,
        now: Instant,
        feeds: &Feeds,
        cfg: &FeedConfig,
        cg_ids: &[&str],
        cmc_ids: &[u32],
        currencies: &[&str],
    ) {
        // Primary: CoinGecko, one batched call for every token. The effective
        // interval is the configured base, or the current backoff while failing.
        let cg_interval = self.cg_backoff.unwrap_or(cfg.coingecko_poll);
        if due(self.cg_last_poll, now, cg_interval) {
            self.cg_last_poll = Some(now);
            match feeds.poll_coingecko(cg_ids) {
                Ok(map) if !map.is_empty() => {
                    self.cg_ok = true;
                    self.cg_backoff = None;
                    if self.cg_logged_down {
                        eprintln!("[feed] coingecko recovered");
                        self.cg_logged_down = false;
                    }
                    for (k, v) in map {
                        self.cg.insert(k, (v, now));
                    }
                }
                // Both empty and errored polls mean CoinGecko can't supply the
                // basis leg this cycle; the engine composes without it (the FX
                // anchor with the last basis, or the static peg), so log the
                // transition once and stay quiet until it recovers rather than
                // spamming a line per tick.
                Ok(_) => {
                    self.cg_ok = false;
                    self.grow_cg_backoff(cfg.coingecko_poll);
                    if !self.cg_logged_down {
                        eprintln!(
                            "[feed] coingecko returned no prices; cascading to the \
                             fallback tier (silencing repeats until it recovers)"
                        );
                        self.cg_logged_down = true;
                    }
                }
                Err(e) => {
                    self.cg_ok = false;
                    self.grow_cg_backoff(cfg.coingecko_poll);
                    if !self.cg_logged_down {
                        eprintln!(
                            "[feed] coingecko poll failed: {e}; cascading to the \
                             fallback tier (silencing repeats until it recovers)"
                        );
                        self.cg_logged_down = true;
                    }
                }
            }
        }

        // Secondary: CoinMarketCap, only when CoinGecko is down and a key is
        // set — the quota rules out a hot poll, so this is a min spacing.
        if !self.cg_ok
            && feeds.coinmarketcap_enabled()
            && !cmc_ids.is_empty()
            && due(self.cmc_last_poll, now, cfg.coinmarketcap_poll)
        {
            self.cmc_last_poll = Some(now);
            match feeds.poll_coinmarketcap(cmc_ids) {
                Ok(map) => {
                    for (k, v) in map {
                        self.cmc.insert(k, (v, now));
                    }
                }
                Err(e) => eprintln!("[feed] coinmarketcap poll failed: {e}"),
            }
        }

        // FX anchor: ECB/Frankfurter USD/<ccy>, keyless, on a slow cadence
        // (the daily reference; the streaming primary is a follow-up).
        if !currencies.is_empty() && due(self.fx_last_poll, now, cfg.fx_poll) {
            self.fx_last_poll = Some(now);
            match feeds.poll_frankfurter(currencies) {
                Ok(map) => {
                    for (k, v) in map {
                        self.fx.insert(k, (v, now));
                    }
                }
                Err(e) => eprintln!("[feed] frankfurter poll failed: {e}"),
            }
        }
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
    feeds: Feeds,
    cfg: BotConfig,
    mut markets: Vec<Context>,
    mut fills: Option<Receiver<Fill>>,
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

    // The batched feed identifiers, collected once. The shared USDC/USD
    // common-mode leg rides the same CoinGecko call as the per-market tokens.
    let mut cg_ids: Vec<&str> = markets.iter().map(|m| m.cfg.coingecko_id).collect();
    cg_ids.push(USDC_COINGECKO_ID);
    let cmc_ids: Vec<u32> = markets
        .iter()
        .filter_map(|m| m.cfg.coinmarketcap_id)
        .collect();
    let mut currencies: Vec<&str> = markets.iter().map(|m| m.cfg.currency).collect();
    currencies.sort_unstable();
    currencies.dedup();

    let mut hub = FeedHub::new();
    loop {
        let now = Instant::now();
        hub.refresh(now, &feeds, &cfg.feeds, &cg_ids, &cmc_ids, &currencies);

        // Drain the one subscription and route each fill to its market.
        let (routed, disconnected) = drain_fills(fills.as_ref(), &markets);
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
/// subscription channel disconnected (the thread died), so the caller can
/// revert every market to the inventory-diff fallback.
///
/// Routing is by market alone: the bootstrap opens exactly one leader vault
/// (sector) per market, and the leader quotes only that sector, so a fill
/// against this leader on this market is unambiguously this vault's. A market
/// with more than one leader-owned sector would need `event.sector_idx`
/// disambiguation too — not a shape this localnet demo creates.
fn drain_fills(
    fills: Option<&Receiver<Fill>>,
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
            Err(TryRecvError::Disconnected) => {
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
    fn poll_is_due_when_never_polled_or_interval_elapsed() {
        let now = Instant::now();
        assert!(due(None, now, Duration::from_secs(10)));
        assert!(!due(Some(now), now, Duration::from_secs(10)));
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
    fn cg_backoff_doubles_from_base_and_caps() {
        let base = Duration::from_secs(60);
        let mut hub = FeedHub::new();
        assert_eq!(hub.cg_backoff, None);
        // First failure starts the backoff at 2× the base.
        hub.grow_cg_backoff(base);
        assert_eq!(hub.cg_backoff, Some(base * 2));
        // Subsequent failures keep doubling, then clamp at the cap.
        for _ in 0..10 {
            hub.grow_cg_backoff(base);
        }
        assert_eq!(hub.cg_backoff, Some(CG_BACKOFF_CAP));
        // A success resets it back to the base.
        hub.cg_backoff = None;
        assert_eq!(hub.cg_backoff, None);
    }
}
