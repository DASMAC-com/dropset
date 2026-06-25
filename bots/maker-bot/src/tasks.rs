//! The tick loop (§3 bot heartbeat).
//!
//! Every tick (5 s): refresh the due feeds, compose the reference, read the
//! live vault, then fire **at most one** instruction — the cold path
//! (`set_liquidity_profile`) takes precedence over the hot path
//! (`set_reference_price`) when a reshape is due, so a tick never sends both.
//! A failed send is logged and the tick is skipped; the next tick retries (no
//! retry storms).
//!
//! Fill detection is driven by the `emit_cpi!` `FillEvent` subscription
//! (`fills` module, §3 production-fidelity path): the subscription thread
//! forwards each attributed fill, the tick drains them into a fill-derived
//! position, and the policy values inventory off that position. The per-tick
//! vault read reconciles the position (catching a missed fill or an external
//! deposit / withdraw the events don't carry) and is the sole fill signal in
//! the fallback path — when no subscription is attached, a `base_atoms` /
//! `quote_atoms` change the bot didn't cause is taken as a fill. (The
//! reference's price-time nonce is *not* used — it bumps on every re-quote, so
//! it can't tell a fill from the bot's own quote update.)
//!
//! Quoting actions stay on the tick boundary (§3: at most one ix per tick, no
//! retry storms); the fill stream makes the bot's inventory *belief*
//! real-time, not its sends.

use crate::chain;
use crate::config::BotConfig;
use crate::context::{Context, ProfileKind, VaultSnapshot};
use crate::model::fair_mid::{compose, Health, Quote};
use crate::model::feeds::Feeds;
use crate::model::inventory::Inventory;
use crate::model::killswitch::{self, Action, HaltReason};
use crate::model::ladder::{self, Side};
use crate::model::skew;
use crate::model::triggers::{self, RefTrigger};
use anyhow::Result;
use solana_signer::Signer;
use std::time::{Duration, Instant};

/// The cached state of one feed: its last successful reading (with the instant
/// it was taken, for freshness) and when it was last polled (for cadence).
#[derive(Default)]
struct FeedState {
    last: Option<(f64, Instant)>,
    last_poll: Option<Instant>,
}

impl FeedState {
    /// Poll the feed if its interval has elapsed, caching a successful reading
    /// and logging a failure without disturbing the last good value.
    fn maybe_poll(
        &mut self,
        now: Instant,
        interval: Duration,
        label: &str,
        poll: impl FnOnce() -> Result<f64>,
    ) {
        let due = self
            .last_poll
            .is_none_or(|t| now.duration_since(t) >= interval);
        if !due {
            return;
        }
        self.last_poll = Some(now);
        match poll() {
            Ok(v) => self.last = Some((v, now)),
            Err(e) => eprintln!("[feed] {label} poll failed: {e}"),
        }
    }

    /// This feed's reading as a [`Quote`] aged to `now`, if any.
    fn quote(&self, now: Instant) -> Option<Quote> {
        self.last.map(|(v, t)| Quote::new(v, now.duration_since(t)))
    }
}

/// Run the bot until interrupted. Each loop iteration is one tick; a tick
/// error is logged and the loop continues.
pub fn run(mut ctx: Context, feeds: Feeds, cfg: BotConfig) -> Result<()> {
    println!(
        "maker-bot live: market {} vault {} (tick {:?})",
        ctx.market.market, ctx.vault_idx, cfg.tick
    );
    let mut cg = FeedState::default();
    let mut fx = FeedState::default();
    let mut ae = FeedState::default();

    loop {
        if let Err(e) = tick(&mut ctx, &feeds, &cfg, &mut cg, &mut fx, &mut ae) {
            eprintln!("[tick] error: {e}");
        }
        std::thread::sleep(cfg.tick);
    }
}

fn tick(
    ctx: &mut Context,
    feeds: &Feeds,
    cfg: &BotConfig,
    cg: &mut FeedState,
    fx: &mut FeedState,
    ae: &mut FeedState,
) -> Result<()> {
    let now = Instant::now();

    // 1. Refresh due feeds.
    cg.maybe_poll(now, cfg.feeds.coingecko_poll, "coingecko", || {
        feeds.poll_coingecko()
    });
    fx.maybe_poll(now, cfg.feeds.oanda_poll, "oanda", || feeds.poll_oanda());
    if feeds.aerodrome_enabled() {
        let interval = cfg.feeds.aerodrome.as_ref().map_or(cfg.tick, |a| a.poll);
        ae.maybe_poll(now, interval, "aerodrome", || feeds.poll_aerodrome());
    }

    // 2. Compose the reference and read the live vault (by quote authority).
    let fair = compose(cg.quote(now), ae.quote(now), fx.quote(now), &cfg.kill);
    let vault = chain::read_vault(&ctx.client, &ctx.market.market, &ctx.leader.pubkey())?;
    ctx.vault_idx = vault.sector_idx;

    // Primary fill signal: drain attributed `FillEvent`s into the position
    // belief, then resolve the inventory the policy reads (reconciling the
    // position against this vault read, or the diff fallback when no
    // subscription is attached).
    drain_fills(ctx, now);
    let (base_atoms, quote_atoms) = resolve_inventory(ctx, &vault, now);

    if vault.frozen {
        println!("[halt] vault is frozen on-chain — idling");
        return Ok(());
    }

    let Some(mid) = fair.mid else {
        println!(
            "[pause] {:?}: no usable CADC source, holding reference",
            fair.health
        );
        return Ok(());
    };

    // 3. Value inventory and decide the action + skewed reference.
    let inv = Inventory::from_atoms(
        base_atoms,
        quote_atoms,
        ctx.market.base_decimals,
        ctx.market.quote_decimals,
        mid,
    );
    let degraded = fair.health == Health::Degraded;
    let action = killswitch::evaluate(&fair, &inv, &cfg.kill, degraded);
    let skew_bps = skew::ref_skew_bps(&inv, &cfg.strategy);
    let reference = skew::apply_skew(mid, skew_bps);

    // 4. Cold path first — at most one ix per tick.
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
            // Already frozen on that side; fall through to the hot path.
        }
        Action::Reshape | Action::Quote => {
            if reshape_due(ctx, cfg, now) {
                arm_standard(ctx, cfg, now)?;
                return Ok(());
            }
        }
    }

    // 5. Hot path — refresh the reference when a trigger fires.
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
            slot,
        )?;
        ctx.last_set_price = Some(reference);
        ctx.last_skew_bps = skew_bps;
        ctx.last_set_at = now;
        println!("[ref] set {reference:.6} (skew {skew_bps:+.1} bps, slot {slot})");
    }
    Ok(())
}

/// Drain every attributed fill delivered since the last tick, log it, and
/// advance the fill-derived position to the latest fill's authoritative
/// `*_after` balances. No-op when no subscription is attached.
fn drain_fills(ctx: &mut Context, now: Instant) {
    let Some(fills) = &ctx.fills else {
        return;
    };
    let mut latest = None;
    for fill in fills.try_iter() {
        let e = &fill.event;
        let side = if e.side == 0 { "ask" } else { "bid" };
        println!(
            "[fill] {side} L{} {} base / {} quote @ {} (fee {} atoms, sig {})",
            e.level_idx, e.fill_base, e.fill_quote, e.fill_price, e.taker_fee_atoms, fill.signature
        );
        latest = Some((e.base_atoms_after, e.quote_atoms_after));
    }
    if let Some((base, quote)) = latest {
        ctx.position = Some((base, quote, now));
    }
}

/// Resolve the inventory the policy values this tick, in `(base, quote)` atoms.
///
/// With a subscription attached, the fill-derived position is authoritative:
/// it is seeded from the first vault read and then advanced by fills, with the
/// per-tick vault read used to reconcile it — a divergence means a missed fill
/// or an external deposit / withdraw the events don't carry, so the chain read
/// wins and the position snaps to it. Without a subscription, the vault read
/// is the only signal and a balance the bot didn't move itself is logged as a
/// fill (the fallback detection).
fn resolve_inventory(ctx: &mut Context, vault: &VaultSnapshot, now: Instant) -> (u64, u64) {
    let chain = (vault.base_atoms, vault.quote_atoms);

    if ctx.fills.is_none() {
        if let Some(prev) = ctx.last_inventory {
            if prev != chain {
                let db = chain.0 as i128 - prev.0 as i128;
                let dq = chain.1 as i128 - prev.1 as i128;
                println!("[fill] inventory moved: base {db:+}, quote {dq:+} atoms");
            }
        }
        ctx.last_inventory = Some(chain);
        return chain;
    }

    match ctx.position {
        None => {
            ctx.position = Some((chain.0, chain.1, now));
            chain
        }
        Some((pb, pq, _)) if (pb, pq) != chain => {
            println!(
                "[fills] reconciling to chain: position ({pb}, {pq}) vs vault ({}, {}) — missed fill or external flow",
                chain.0, chain.1
            );
            ctx.position = Some((chain.0, chain.1, now));
            chain
        }
        Some((pb, pq, _)) => (pb, pq),
    }
}

/// Whether the standard ladder needs re-arming this tick — either it isn't the
/// armed shape (first tick, or recovering from a halt/freeze) or the daily
/// heartbeat is due.
fn reshape_due(ctx: &Context, cfg: &BotConfig, now: Instant) -> bool {
    ctx.profile_kind != ProfileKind::Standard
        || triggers::should_set_profile_heartbeat(
            now.duration_since(ctx.last_profile_at),
            cfg.strategy.profile_heartbeat,
        )
}

/// Arm the full symmetric ladder.
fn arm_standard(ctx: &mut Context, cfg: &BotConfig, now: Instant) -> Result<()> {
    let profile = ladder::build_profile(&cfg.strategy.ladder);
    chain::set_liquidity_profile(
        &ctx.client,
        &ctx.leader,
        &ctx.market.market,
        ctx.vault_idx,
        ladder::to_bytes(&profile),
    )?;
    ctx.profile_kind = ProfileKind::Standard;
    ctx.last_profile_at = now;
    println!("[profile] armed standard ladder");
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
        ladder::to_bytes(&profile),
    )?;
    ctx.profile_kind = ProfileKind::FrozenSide(side);
    ctx.last_profile_at = Instant::now();
    println!("[freeze] zeroed {side:?} side — only the rebuild side quotes");
    Ok(())
}

/// Stop quoting and alert. The bot zeroes both sides (leader-authorized) and
/// leaves the irreversible, admin-only `FreezeVault` to a human.
fn halt(ctx: &mut Context, cfg: &BotConfig, reason: HaltReason) -> Result<()> {
    eprintln!("[ALERT] kill switch: {reason:?} — halting quotes for review");
    if ctx.profile_kind != ProfileKind::Halted {
        let mut profile = ladder::build_profile(&cfg.strategy.ladder);
        ladder::zero_side(&mut profile, Side::Bid);
        ladder::zero_side(&mut profile, Side::Ask);
        chain::set_liquidity_profile(
            &ctx.client,
            &ctx.leader,
            &ctx.market.market,
            ctx.vault_idx,
            ladder::to_bytes(&profile),
        )?;
        ctx.profile_kind = ProfileKind::Halted;
        ctx.last_profile_at = Instant::now();
        println!("[halt] zeroed both sides; existing levels expire on their own");
    }
    Ok(())
}
