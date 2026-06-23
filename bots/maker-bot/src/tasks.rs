//! The tick loop (§3 bot heartbeat).
//!
//! Every tick (5 s): refresh the due feeds, compose the reference, read the
//! live vault, then fire **at most one** instruction — the cold path
//! (`set_liquidity_profile`) takes precedence over the hot path
//! (`set_reference_price`) when a reshape is due, so a tick never sends both.
//! A failed send is logged and the tick is skipped; the next tick retries (no
//! retry storms).
//!
//! Fill detection for the MVP rides the per-tick vault read: the reference's
//! price-time nonce bumps on every flush, so a change since the last tick
//! means a fill landed. The spec's `emit_cpi` event subscription is the
//! production-fidelity path and is deferred (the adversarial taker that would
//! exercise it is itself deferred, §5).

use crate::chain;
use crate::config::BotConfig;
use crate::context::{Context, ProfileKind};
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

    // Fill detection: a nonce bump since the last tick means a flush landed.
    if ctx.last_nonce != 0 && vault.nonce != ctx.last_nonce {
        println!(
            "[fill] vault nonce {} → {} (a level filled)",
            ctx.last_nonce, vault.nonce
        );
    }
    ctx.last_nonce = vault.nonce;

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
        vault.base_atoms,
        vault.quote_atoms,
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
