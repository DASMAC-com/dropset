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
use crate::config::{BotConfig, MarketConfig, USDC_COINGECKO_ID, USDC_KRAKEN_PAIR};
use crate::context::{Context, ProfileKind, VaultSnapshot};
use crate::fills::Fill;
use crate::model::fair_mid::{build_legs, FairValue};
use crate::model::invalidate::{self, InvalidateReason};
use crate::model::inventory::Inventory;
use crate::model::killswitch::{self, Action};
use crate::model::ladder::{self, Side};
use crate::model::skew;
use crate::model::triggers::{self, RefTrigger};
use anyhow::Result;
use dropset_fair_value::{Legs, Reading};
use dropset_feeds::venues::FxQuote;
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
    /// Pyth Hermes — the primary FX anchor, carrying a confidence half-width
    /// and a publish time per reading.
    pub pyth: broadcast::Receiver<HashMap<String, FxQuote>>,
    /// Kraken — the batched basis secondary and the USDC/USD peg-truth leg.
    pub kraken: broadcast::Receiver<HashMap<String, f64>>,
    /// Coinbase spot tickers — the primary basis leg, one source per product,
    /// all forwarded onto one channel keyed by product id. Empty when no
    /// selected market is listed on Coinbase.
    pub coinbase: Vec<broadcast::Receiver<(String, f64)>>,
    pub coingecko: broadcast::Receiver<HashMap<String, f64>>,
    pub coinmarketcap: Option<broadcast::Receiver<HashMap<u32, f64>>>,
    pub frankfurter: broadcast::Receiver<HashMap<String, f64>>,
}

/// Drain every reading queued on `rx` into `cache`, stamping `now` as the read
/// time the engine's freshness rules age from. Stops on an empty or closed
/// channel (a closed source's last reading is left to age out); a lag — the
/// source outran a slow cycle — skips to the retained latest and keeps
/// draining, since the freshest reading is the one the cache wants.
fn drain_into<K: Eq + Hash + Clone, V: Clone>(
    rx: &mut broadcast::Receiver<HashMap<K, V>>,
    cache: &mut HashMap<K, (V, Instant)>,
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

/// Drain a stream of `(key, value)` singletons — the shape a per-product source
/// yields — into `cache`. The batched-map counterpart is [`drain_into`].
///
/// Named "entries" rather than "pairs" deliberately: in this module a *pair* is
/// a Kraken trading pair ([`USDC_KRAKEN_PAIR`], `MarketConfig::kraken_pair`),
/// and reusing the word for a key/value tuple reads as the wrong thing three
/// lines from where the other sense is used.
fn drain_entries_into(
    rx: &mut broadcast::Receiver<(String, f64)>,
    cache: &mut HashMap<String, (f64, Instant)>,
    now: Instant,
) {
    loop {
        match rx.try_recv() {
            Ok((k, v)) => {
                cache.insert(k, (v, now));
            }
            Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            Err(TryRecvError::Lagged(_)) => continue,
        }
    }
}

/// The shared feed cache. Each tier's source polls on its own cadence and
/// forwards its readings onto a live sink; a cycle drains those into the
/// per-tier caches below, and `legs()` composes each market's legs by walking
/// the tiers in preference order and taking the first that answers.
///
/// The tier table itself lives with the legs it feeds, in
/// [`crate::model::fair_mid`] — this walks it, it does not define it.
struct FeedHub {
    /// `currency → (USD per unit + confidence + publish time, when read)`.
    pyth: HashMap<String, (FxQuote, Instant)>,
    /// `kraken pair → (price, when read)`.
    kraken: HashMap<String, (f64, Instant)>,
    /// `coinbase product id → (USDC per token, when read)`.
    coinbase: HashMap<String, (f64, Instant)>,
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
            pyth: HashMap::new(),
            kraken: HashMap::new(),
            coinbase: HashMap::new(),
            cg: HashMap::new(),
            cmc: HashMap::new(),
            fx: HashMap::new(),
        }
    }

    /// Drain each tier's live-sink receiver into the cache, stamping `now`. The
    /// CoinMarketCap tier is drained only when it's wired up this run.
    fn drain(&mut self, now: Instant, rx: &mut FeedReceivers) {
        drain_into(&mut rx.pyth, &mut self.pyth, now);
        drain_into(&mut rx.kraken, &mut self.kraken, now);
        for product in &mut rx.coinbase {
            drain_entries_into(product, &mut self.coinbase, now);
        }
        drain_into(&mut rx.coingecko, &mut self.cg, now);
        if let Some(cmc) = rx.coinmarketcap.as_mut() {
            drain_into(cmc, &mut self.cmc, now);
        }
        drain_into(&mut rx.frankfurter, &mut self.fx, now);
    }

    /// This market's cached readings, aged to `now`, mapped onto the engine's
    /// [`Legs`] (§1) by walking each leg's tiers in the order
    /// [`crate::model::fair_mid`] tables.
    fn legs(&self, market: &MarketConfig, tick: &TickCtx) -> Legs {
        let now = tick.now;
        let aged =
            |o: Option<&(f64, Instant)>| o.map(|(v, t)| Reading::new(*v, now.duration_since(*t)));

        // Any tier with another tier beneath it is read through `live`, which
        // drops a reading the engine would reject as stale so the walk falls
        // through instead of pinning the leg on a dead source. Tiering on
        // *presence* alone is not merely imprecise here: these caches never
        // evict, so one source dying once masks every tier below it for the life
        // of the process rather than self-healing on the next tick.
        //
        // The **last** tier of each leg is deliberately read with bare `aged`.
        // Nothing sits below it to fall through to, and the engine's own
        // `Reading::fresh` check degrades the market on a stale leg downstream —
        // so gating it would only turn "stale" into "missing" and throw away the
        // reading's age on the way. The FX anchor below is the same shape: Pyth
        // gated, Frankfurter not.
        let live = |o: Option<&(f64, Instant)>| aged(o).filter(|r| r.fresh(tick.leg_stale));

        // FX anchor. Pyth is preferred for two reasons: it publishes a
        // confidence half-width (the fresh-but-uncertain regime, §1 fm6, is
        // unobservable without one), and it is aged from the publisher's own
        // clock — see `pyth_reading`.
        //
        // The hand-off to Frankfurter is gated on the **engine's own**
        // staleness bound, not on some looser ceiling: a Pyth reading the
        // engine would reject as stale must not sit in the slot and mask a
        // live fallback, or a 20-minute Hermes outage would dark the anchor
        // while a perfectly good ECB rate went unread.
        let fx_pyth = self
            .pyth
            .get(market.currency)
            .map(|(q, t)| pyth_reading(q, *t, now, tick.now_unix))
            .filter(|r| r.fresh(tick.leg_stale));

        // …and the fallback is suppressed while the FX session is closed.
        // Frankfurter is aged from *receipt*, so it reads fresh all weekend
        // even though ECB published its last rate on Friday. Letting it stand
        // in would keep the engine in the Normal regime on a dead market —
        // precisely the "fall back to a stale peg" behavior §1 fm2 rejects in
        // favor of switching the anchor to the crypto reference.
        let fx = match (fx_pyth, tick.weekend) {
            (Some(r), _) => Some(r),
            (None, true) => None,
            (None, false) => aged(self.fx.get(market.currency)),
        };

        // USDC/USD common-mode leg, shared across every market.
        let usdc_usd = live(self.kraken.get(USDC_KRAKEN_PAIR))
            .or_else(|| aged(self.cg.get(USDC_COINGECKO_ID)));

        // Crypto basis leg, in **USDC per token**. Coinbase quotes that
        // directly. Kraken quotes the token in *USD*, so it is converted with
        // the peg leg above rather than assumed equal: the `usdc_usd` guard
        // only *alarms* at a 3% deviation, it does not correct one, and
        // leaving it uncorrected would make the basis jump whenever the tier
        // flipped between Coinbase and Kraken. CoinGecko and CMC are the
        // reflexive last resort (§1 fm5) and carry the same USD-for-USDC
        // approximation as before — untouched here.
        let usdc_per_usd = usdc_usd.map(|r| r.value).filter(|v| *v > 0.0);
        let crypto_usdc = market
            .coinbase_product
            .and_then(|p| live(self.coinbase.get(p)))
            .or_else(|| {
                market.kraken_pair.and_then(|p| {
                    live(self.kraken.get(p)).map(|r| match usdc_per_usd {
                        Some(peg) => Reading {
                            value: r.value / peg,
                            ..r
                        },
                        None => r,
                    })
                })
            })
            .or_else(|| market.coingecko_id.and_then(|id| live(self.cg.get(id))))
            .or_else(|| {
                market
                    .coinmarketcap_id
                    .and_then(|id| aged(self.cmc.get(&id)))
            });

        build_legs(fx, crypto_usdc, usdc_usd, market.static_usd)
    }
}

/// The per-tick inputs [`FeedHub::legs`] needs beyond the cache itself,
/// bundled so the tiering reads the same clock and the same bounds the engine
/// will apply a moment later.
struct TickCtx {
    /// Monotonic read time every cached reading is aged against.
    now: Instant,
    /// The same instant on the wall clock, for `publish_time` arithmetic.
    now_unix: i64,
    /// The engine's per-leg staleness bound. The tiering needs it so a stale
    /// primary hands off instead of masking its fallback.
    leg_stale: Duration,
    /// Whether the FX session is closed (§1 fm2) — suppresses the receipt-aged
    /// FX fallback so the crypto-only regime can engage.
    weekend: bool,
}

/// A ceiling on the age [`pyth_reading`] will report, so a wildly skewed clock
/// or a bogus `publish_time` degrades to "stale" rather than to a negative or
/// absurd duration. Well past `leg_stale`, so it only ever bites on nonsense.
const MAX_PYTH_AGE: Duration = Duration::from_secs(24 * 3600);

/// How far ahead of this host's clock a `publish_time` may sit before it is
/// treated as bogus rather than as "just published". Ordinary NTP skew between
/// the publishers and us is sub-second; a minute is generous.
const MAX_PYTH_CLOCK_SKEW: Duration = Duration::from_secs(60);

/// Turn a cached Pyth quote into a [`Reading`], aged from the **publisher's**
/// clock rather than from when this process received it.
///
/// This is the whole reason the FX anchor tracks `publish_time`. Pyth's FX
/// feeds follow the interbank schedule and stop publishing over the weekend, so
/// a reading aged from receipt would show a frozen Friday-close rate as
/// perpetually fresh and the weekend crypto-only regime (§1 fm2) would never
/// engage. The receipt age is still taken as a floor: if the poller itself dies
/// the leg has to go stale even if the last `publish_time` looked recent.
///
/// **A `publish_time` in the future is bogus, not fresh.** It is venue-supplied
/// data being used as a clock, so the forward direction has to be bounded too —
/// otherwise a stamp an hour (or a century) ahead pins the age at zero and a
/// frozen rate reads as perpetually fresh, which is the exact failure this
/// function exists to prevent. Past a minute of tolerated skew the stamp is
/// discarded in favor of the receipt age.
fn pyth_reading(q: &FxQuote, read_at: Instant, now: Instant, now_unix: i64) -> Reading {
    let delta = now_unix.saturating_sub(q.publish_time);
    let received = now.duration_since(read_at);
    let published = if delta < -(MAX_PYTH_CLOCK_SKEW.as_secs() as i64) {
        // Implausibly far ahead of us — trust nothing it says about its age.
        MAX_PYTH_AGE
    } else {
        Duration::from_secs(delta.max(0) as u64)
    };
    let age = published.max(received).min(MAX_PYTH_AGE);
    match q.confidence {
        Some(conf) => Reading::with_confidence(q.value, age, conf),
        None => Reading::new(q.value, age),
    }
}

/// Whether the Unix timestamp `secs` falls in the FX-closed weekend window.
/// Interbank FX and CME 6E are shut Fri ~17:00 → Sun ~17:00 ET (§1 fm2);
/// approximated here in UTC as Fri 21:00 → Sun 22:00 (≈ 17:00 ET, ignoring
/// DST). The exact session thresholds are TBD(analytics). Inside this window a
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

/// The wall clock as an epoch second. A clock before the Unix epoch
/// (unreachable in practice) reads as zero.
fn unix_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// [`weekend_from_unix`] for the wall clock.
fn is_weekend(now: SystemTime) -> bool {
    weekend_from_unix(unix_secs(now))
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

    // Declare, once at wiring time, every market quoting on a pinned basis. A
    // market with no independent basis source is a standing condition rather
    // than an event, so it is stated at startup instead of re-alarmed per tick.
    for ctx in &markets {
        if let Some(b) = ctx.cfg.pinned_basis {
            println!(
                "[basis] {}: no independent basis source — quoting the FX \
                 anchor with a pinned basis of {b:.4} (unverified)",
                ctx.cfg.symbol
            );
        }
    }

    // Before the first quote of the run: kill any book still resting at a stale
    // price from a previous run (or from before a chain halt). This must precede
    // the loop — takers can fill against those levels from the first block the
    // bot is back, and the earliest a fresh reference could land is one cycle in.
    invalidate_stale_quotes(&mut markets, &cfg);

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

        // The FX session is closed the same wall-clock window for every market,
        // and the same second dates every Pyth reading this cycle.
        let wall = SystemTime::now();
        let weekend = is_weekend(wall);
        let tick = TickCtx {
            now,
            now_unix: unix_secs(wall) as i64,
            leg_stale: cfg.fair_value.leg_stale,
            weekend,
        };
        for ctx in &mut markets {
            let legs = hub.legs(&ctx.cfg, &tick);
            check_first_basis(ctx, &cfg, legs);
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

/// Validate a market's **first observable basis** against the sane band, once
/// per run, and say so loudly when it lands outside.
///
/// The band exists to catch a *peg event* (§4) — a token that was tracking its
/// fiat and stopped. But the very first observation cannot be that: there is no
/// "was tracking" to depart from. A basis that is already outside the band the
/// first time it is computed means the two legs are not measuring the same
/// thing — a mismatched index id, a token/USD reading mistaken for token/USDC,
/// an inverted FX feed. Those are wiring errors, and they are silent today: the
/// tick loop simply reports a breach every tick forever, which reads as a
/// market event and trains the operator to ignore the alarm that matters.
///
/// So this reports at most once, names the configuration as the suspect, and
/// does **not** halt: from a single reading the bot cannot tell a misconfigured
/// market from one whose peg has genuinely broken, and refusing to boot the
/// whole roster over one market's feed would be the worse failure. The breach path
/// still runs — this only ensures the first one is attributed correctly.
///
/// A market with a pinned basis is skipped: it has no observation to check, and
/// its unverified state is already declared at startup.
///
/// **The latch is per market, not per source tier**, which bounds what it can
/// catch. A market whose CEX primary answers first spends its shot on that
/// reading, so the index ids further down its ladder are never validated — and
/// a mis-wired index id is reachable only *through* that fallback. On this
/// roster only EURC has a primary at all, so the other five are checked on the
/// tier that actually prices them; but EURC's fallback ids would go unchecked
/// until the day it falls back, which is the day the per-tick breach path fires
/// and reads as a peg event. Latching per tier would close it, and belongs with
/// the multi-source work rather than here.
fn check_first_basis(ctx: &mut Context, cfg: &BotConfig, legs: Legs) {
    if ctx.basis_checked || ctx.cfg.pinned_basis.is_some() {
        return;
    }
    let Some(observed) = observable_basis(legs, cfg.fair_value.leg_stale) else {
        return;
    };
    ctx.basis_checked = true;
    let (low, high) = (cfg.fair_value.basis_low, cfg.fair_value.basis_high);
    if observed < low || observed > high {
        eprintln!(
            "[basis] {}: first observed basis {observed:.4} is outside the sane \
             band [{low:.2}, {high:.2}] — a first reading cannot be a peg event, \
             so treat this as a feed/config error (wrong index id, or a \
             token/USD reading used as token/USDC) rather than a market move",
            ctx.cfg.symbol
        );
    }
}

/// The basis a pair of legs implies, when one is observable at all: both legs
/// live, fresh, and the FX anchor positive. `None` means there is nothing to
/// check this tick — the sources are still warming, or one has dropped out.
///
/// Split out from [`check_first_basis`] so the observability rule is testable
/// without standing up a [`Context`] (which needs an RPC client and a keypair).
fn observable_basis(legs: Legs, stale: Duration) -> Option<f64> {
    let fx = legs.fx.filter(|r| r.fresh(stale))?;
    let crypto = legs.crypto_usdc.filter(|r| r.fresh(stale))?;
    (fx.value > 0.0).then(|| crypto.value / fx.value)
}

/// Kill every market's resting book that is too stale to leave matchable,
/// before the supervisor quotes for the first time.
///
/// This is the unconditional half of the halt / pick-off mitigation: it holds
/// whatever the reason for the gap was — the bot crashed, the operator restarted
/// it, or the chain itself halted — because it reasons about the bot's own
/// wall-clock record rather than about slots, which stop ticking during a halt.
///
/// Per-market failures are logged and skipped rather than propagated: one market
/// whose RPC read or kill stamp fails must not stop the other six from being
/// made safe, and the running paths that also call [`invalidate_resting_book`]
/// pick up whatever this pass couldn't land.
fn invalidate_stale_quotes(markets: &mut [Context], cfg: &BotConfig) {
    let now = SystemTime::now();
    for ctx in markets {
        let vault = match chain::read_vault(
            &ctx.client,
            &ctx.market.market,
            &ctx.leader.pubkey(),
            ctx.market.base_decimals,
            ctx.market.quote_decimals,
        ) {
            Ok(vault) => vault,
            Err(e) => {
                eprintln!(
                    "[{}][invalidate] vault read failed, cannot check for stale quotes: {e}",
                    ctx.cfg.symbol
                );
                continue;
            }
        };
        ctx.vault_idx = vault.sector_idx;
        // A frozen vault is already skipped by the matcher, so its resting book
        // can't be hit and the stamp would be a wasted priority-fee transaction.
        // (`reference_valid` deliberately tracks only the price half of the
        // program's gate, so the frozen half is checked here — the same order
        // `quote_market` uses.)
        if vault.frozen {
            continue;
        }
        let age = ctx.quote_state.age(now);
        if let Err(e) = invalidate_resting_book(ctx, cfg, &vault, age, InvalidateReason::Startup) {
            eprintln!("[{}][invalidate] kill stamp failed: {e}", ctx.cfg.symbol);
        }
    }
}

/// Send the kill stamp **unconditionally** and note it. The gated entry point is
/// [`invalidate_resting_book`] — go through that unless you have already
/// established that the stamp is warranted, or you will bypass both the
/// staleness rule and the once-per-episode guard.
///
/// `age` is only for the log line; the decision is the caller's.
fn send_kill_stamp(
    ctx: &mut Context,
    cfg: &BotConfig,
    reason: InvalidateReason,
    age: Option<Duration>,
) -> Result<()> {
    let slot = chain::current_slot(&ctx.client)?;
    chain::invalidate_reference_price(
        &ctx.client,
        &ctx.leader,
        &ctx.market.market,
        ctx.vault_idx,
        slot,
        cfg.invalidate.priority_micro_lamports,
    )?;
    ctx.reference_invalidated = true;
    // The stamped price is gone, so the cadence state describing it is stale
    // too. Clearing it puts this market back in its first-cycle shape, where
    // `should_set_reference` fires unconditionally — otherwise the drift and
    // heartbeat arms would both decline (the last stamp is recent, and drift
    // against it is small) and the vault would sit dark with a live ladder until
    // the next heartbeat, up to `ref_heartbeat` after the cause had cleared.
    ctx.last_set_price = None;
    let evidence = age.map_or_else(
        || "no fresh-quote evidence".to_string(),
        |age| format!("last live quote {}s old", age.as_secs()),
    );
    println!(
        "[{}][invalidate] {reason:?}: {evidence} — stamped the kill price; the book is \
         unmatchable until the next fresh quote",
        ctx.cfg.symbol
    );
    Ok(())
}

/// Kill this market's resting book if `age` leaves it too stale to sit
/// matchable. The single decision point every path routes through — the startup
/// pass, the no-usable-feed pause, and the kill-switch halt — so the "is it
/// worth an instruction" rule lives in exactly one place
/// ([`invalidate::should_invalidate`]).
///
/// `age` is the freshness evidence: the persisted record's age at startup, the
/// in-run gap since the last stamp on the pause path, and `None` on a halt,
/// where the bot has decided to stop standing behind the price and so has no
/// freshness to claim.
/// Returns whether it spent this cycle's instruction on the kill stamp, so a
/// caller with a second instruction to send can order the two.
fn invalidate_resting_book(
    ctx: &mut Context,
    cfg: &BotConfig,
    vault: &VaultSnapshot,
    age: Option<Duration>,
    reason: InvalidateReason,
) -> Result<bool> {
    // Already killed this episode. The send confirms before returning, so
    // `reference_valid` on the next cycle's read would catch it too; this flag
    // just saves re-deciding it every cycle for as long as the episode lasts.
    if ctx.reference_invalidated {
        return Ok(false);
    }
    if !invalidate::should_invalidate(vault.reference_valid, age, cfg.invalidate.stale_after) {
        return Ok(false);
    }
    send_kill_stamp(ctx, cfg, reason, age)?;
    Ok(true)
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
        // Holding the reference is only safe while it is still roughly right.
        // Once the outage outlasts the staleness bound, take the book dark
        // instead of leaving it matchable at a price nothing is refreshing.
        // Ages off `last_set_at` (monotonic) rather than the persisted record:
        // within a run the bot knows exactly when it last stamped, and an
        // `Instant` can't be walked backwards by a clock adjustment.
        invalidate_resting_book(
            ctx,
            cfg,
            &vault,
            Some(now.duration_since(ctx.last_set_at)),
            InvalidateReason::QuotesStale,
        )?;
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

    let action = killswitch::evaluate(&fair, &inv, &cfg.kill, launch_tvl);
    let skew_bps = skew::ref_skew_bps(&inv, &cfg.strategy);
    let reference = skew::apply_skew(mid, skew_bps);

    // Cold path first — at most one ix per cycle.
    match action {
        Action::Halt(reason) => {
            eprintln!(
                "[{}][ALERT] kill switch: {reason:?} — halting quotes for review",
                ctx.cfg.symbol
            );
            // Standing down fully takes two instructions and the cycle budget is
            // one, so send them in order of what actually protects the vault.
            // The kill stamp takes the resting book out of the matching set;
            // zeroing the profile only stops the *next* flush from materializing
            // more levels, which does nothing about what is already resting. So
            // the stamp goes first — a halt fires exactly when a stale price is
            // most worth picking off, and deferring the stamp a whole cycle
            // leaves the book live through it.
            //
            // Ordering it this way also keeps the profile send from starving it:
            // that send propagates its error and only records `Halted` on
            // success, so a persistently failing cold path would otherwise retry
            // forever and the book would never go dark at all.
            if !invalidate_resting_book(ctx, cfg, &vault, None, InvalidateReason::Halted)? {
                zero_both_sides(ctx, cfg)?;
            }
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
        // A live reference re-arms whatever the kill stamp took dark, so the
        // episode is over.
        ctx.reference_invalidated = false;
        // Write down the wall-clock time this book was last correctly priced, so
        // a restart can age it. A failed write only costs the *next* startup its
        // freshness evidence, which reads as stale — the safe direction — so it
        // must not fail the tick.
        if let Err(e) = ctx.quote_state.record(SystemTime::now()) {
            eprintln!(
                "[{}][invalidate] could not persist the quote timestamp \
                 (a restart will treat this book as stale): {e}",
                ctx.cfg.symbol
            );
        }
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

/// Zero both sides of the ladder so no future flush materializes a level — the
/// leader-authorized half of a halt, leaving the irreversible, admin-only
/// `FreezeVault` to a human.
///
/// This is the *second* of the two instructions a halt sends; the caller sends
/// the kill stamp first (see the `Action::Halt` arm), because zeroing the profile
/// does nothing about the levels already resting. A no-op once the shape is
/// already `Halted`, so repeat cycles cost nothing.
fn zero_both_sides(ctx: &mut Context, cfg: &BotConfig) -> Result<()> {
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
            "[{}][halt] zeroed both sides; the resting book was already killed",
            ctx.cfg.symbol
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MARKETS;
    use crate::context::MarketAddrs;
    use crate::quote_state::QuoteStateStore;
    use dropset_fair_value::{FairValueConfig, Health, Regime};
    use solana_keypair::Keypair;

    /// A `Context` that never talks to a validator. `RpcClient::new` doesn't
    /// connect, so this is only unsound if the code under test actually sends —
    /// which is exactly what the guard tests below assert it does *not* do.
    fn offline_ctx(dir: &std::path::Path) -> Context {
        offline_ctx_with(dir, MARKETS[0])
    }

    /// The same offline context for an arbitrary market, so a test can exercise
    /// the per-market calibration `Context::new` layers onto the shared config —
    /// which mutating `ctx.cfg` after the fact cannot, since the engine is
    /// already built by then.
    fn offline_ctx_with(dir: &std::path::Path, cfg: MarketConfig) -> Context {
        let market = MarketAddrs {
            market: Pubkey::new_unique(),
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            base_decimals: 6,
            quote_decimals: 6,
        };
        let quote_state = QuoteStateStore::new(dir).for_market(market.market, cfg.symbol);
        Context::new(
            chain::rpc("http://127.0.0.1:1"),
            Keypair::new(),
            0,
            market,
            cfg,
            FairValueConfig::default(),
            quote_state,
        )
    }

    fn snapshot(reference_valid: bool) -> VaultSnapshot {
        VaultSnapshot {
            sector_idx: 0,
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            reference_price: 1.14,
            reference_valid,
            frozen: false,
        }
    }

    /// The gated entry point must decline — without sending, hence without
    /// touching the network — in each of the three cases that don't warrant a
    /// stamp. `send_kill_stamp` is what would hit the RPC, so an offline client
    /// is enough to prove the guards return first: a regression here fails by
    /// erroring on the bogus endpoint rather than by asserting false.
    #[test]
    fn the_gate_declines_without_sending() {
        let dir = std::env::temp_dir().join("dropset-invalidate-gate");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = BotConfig::default();

        // 1. Already killed this episode.
        let mut ctx = offline_ctx(&dir);
        ctx.reference_invalidated = true;
        assert!(!invalidate_resting_book(
            &mut ctx,
            &cfg,
            &snapshot(true),
            None,
            InvalidateReason::Halted
        )
        .expect("the dedup guard returns before any send"));

        // 2. The book is already dark, so there is nothing to kill.
        let mut ctx = offline_ctx(&dir);
        assert!(!invalidate_resting_book(
            &mut ctx,
            &cfg,
            &snapshot(false),
            None,
            InvalidateReason::Startup
        )
        .expect("an invalid reference returns before any send"));

        // 3. The quote is still fresh.
        let mut ctx = offline_ctx(&dir);
        assert!(!invalidate_resting_book(
            &mut ctx,
            &cfg,
            &snapshot(true),
            Some(Duration::from_secs(1)),
            InvalidateReason::QuotesStale
        )
        .expect("a fresh quote returns before any send"));
        assert!(!ctx.reference_invalidated, "no episode was opened");
    }

    /// The startup check only spends its one shot on a basis that actually
    /// exists: the feed sources warm asynchronously, so the early ticks have a
    /// partial or empty leg set and must not count as "checked".
    #[test]
    fn a_basis_is_observable_only_when_both_legs_are_live_and_fresh() {
        let stale = Duration::from_secs(300);
        let fresh = |v: f64| Some(Reading::new(v, Duration::from_secs(1)));
        let base = Legs {
            fx: fresh(0.0573),
            crypto_usdc: fresh(0.0573),
            usdc_usd: fresh(1.0),
            static_usd: 0.0573,
        };

        assert_eq!(observable_basis(base, stale), Some(1.0));
        // Either leg missing — nothing to check yet.
        assert_eq!(observable_basis(Legs { fx: None, ..base }, stale), None);
        assert_eq!(
            observable_basis(
                Legs {
                    crypto_usdc: None,
                    ..base
                },
                stale
            ),
            None
        );
        // A leg present but stale is not a reading.
        assert_eq!(
            observable_basis(
                Legs {
                    fx: Some(Reading::new(0.0573, Duration::from_secs(600))),
                    ..base
                },
                stale
            ),
            None
        );
        // A non-positive anchor would divide by zero.
        assert_eq!(
            observable_basis(
                Legs {
                    fx: fresh(0.0),
                    ..base
                },
                stale
            ),
            None
        );
        // The MXNe shape the issue reported: a basis near 0.52.
        let observed = observable_basis(
            Legs {
                crypto_usdc: fresh(0.03064),
                ..base
            },
            stale,
        )
        .unwrap();
        assert!((observed - 0.5347).abs() < 1e-3, "observed {observed}");
    }

    /// A pinned market has no observation to validate, so the check must not
    /// consume its one shot — and must never report a band violation for a
    /// constant that was never measured.
    #[test]
    fn the_startup_basis_check_skips_a_pinned_market() {
        let dir = std::env::temp_dir().join("dropset-basis-check-pinned");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = BotConfig::default();
        let legs = Legs {
            fx: Some(Reading::new(0.0573, Duration::from_secs(1))),
            // Deliberately garbage: were this market not pinned, it would trip.
            crypto_usdc: Some(Reading::new(0.03064, Duration::from_secs(1))),
            usdc_usd: Some(Reading::new(1.0, Duration::from_secs(1))),
            static_usd: 0.0573,
        };

        let mut pinned = offline_ctx(&dir);
        pinned.cfg.pinned_basis = Some(1.0);
        check_first_basis(&mut pinned, &cfg, legs);
        assert!(
            !pinned.basis_checked,
            "a pinned market has nothing to check"
        );

        // The same legs on an unpinned market do consume the shot, once.
        let mut observed = offline_ctx(&dir);
        observed.cfg.pinned_basis = None;
        check_first_basis(&mut observed, &cfg, legs);
        assert!(observed.basis_checked);
    }

    /// The per-market pin has to survive the trip through `Context::new`, which
    /// layers it onto the *shared* calibration. Asserting it here rather than on
    /// the config constant is the point: the roster invariant test proves MXNe
    /// declares a pin, and this proves the engine that market actually quotes
    /// with honours it. Without this, dropping the layering line would leave
    /// MXNe silently back on `Degrade::NoBasisLeg` with every other test green.
    #[test]
    fn a_pinned_market_context_composes_in_the_pinned_regime() {
        let dir = std::env::temp_dir().join("dropset-basis-check-layering");
        let _ = std::fs::remove_dir_all(&dir);
        let mxne = *MARKETS.iter().find(|m| m.symbol == "MXNe").unwrap();
        assert!(mxne.pinned_basis.is_some(), "MXNe is the pinned market");

        let mut ctx = offline_ctx_with(&dir, mxne);
        let legs = Legs {
            fx: Some(Reading::new(0.0573, Duration::from_secs(1))),
            crypto_usdc: None,
            usdc_usd: None,
            static_usd: mxne.static_usd,
        };
        let fair = ctx.engine.compose(legs, Duration::from_secs(1), false);
        assert_eq!(fair.regime, Regime::FxPinned);
        assert_eq!(fair.health, Health::Unverified);
        assert!(!fair.basis_breach);

        // The converse, so the assertion above cannot pass vacuously: an
        // unpinned market built the same way composes normally.
        let eurc = *MARKETS.iter().find(|m| m.symbol == "EURC").unwrap();
        let mut ctx = offline_ctx_with(&dir, eurc);
        let fair = ctx.engine.compose(legs, Duration::from_secs(1), false);
        assert_ne!(fair.regime, Regime::FxPinned);
    }

    /// An empty leg set leaves the shot unspent, so the check still fires on
    /// the first tick that has a basis rather than being burned while warming.
    #[test]
    fn the_startup_basis_check_waits_for_a_live_basis() {
        let dir = std::env::temp_dir().join("dropset-basis-check-warming");
        let _ = std::fs::remove_dir_all(&dir);
        let cfg = BotConfig::default();
        let mut ctx = offline_ctx(&dir);
        ctx.cfg.pinned_basis = None;

        check_first_basis(&mut ctx, &cfg, Legs::default());
        assert!(!ctx.basis_checked, "nothing was observable yet");

        check_first_basis(
            &mut ctx,
            &cfg,
            Legs {
                fx: Some(Reading::new(1.14, Duration::from_secs(1))),
                crypto_usdc: Some(Reading::new(1.14, Duration::from_secs(1))),
                usdc_usd: None,
                static_usd: 1.14,
            },
        );
        assert!(ctx.basis_checked);
    }

    /// A context with no persisted record starts its hot-path clock at "now" —
    /// the fallback arm of the seeding in `Context::new`.
    #[test]
    fn an_unknown_record_starts_the_clock_at_now() {
        let dir = std::env::temp_dir().join("dropset-invalidate-clock-unknown");
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = offline_ctx(&dir);
        assert!(Instant::now().duration_since(ctx.last_set_at) < Duration::from_secs(1));
    }

    /// With a persisted record, the clock is seeded *back* by the record's age,
    /// so the running-path staleness check measures the resting book's real age
    /// across a restart instead of re-crediting it a full bound.
    #[test]
    fn a_persisted_record_seeds_the_clock_backwards() {
        let dir = std::env::temp_dir().join("dropset-invalidate-clock-seeded");
        let _ = std::fs::remove_dir_all(&dir);
        let market = Pubkey::new_unique();
        let store = QuoteStateStore::new(&dir);
        // A stamp 55 s ago: inside the 60 s bound, so the startup pass would
        // decline — which is precisely the case that used to reset the clock.
        store
            .for_market(market, "EURC")
            .record(SystemTime::now() - Duration::from_secs(55))
            .expect("record the stamp");
        let addrs = MarketAddrs {
            market,
            base_mint: Pubkey::new_unique(),
            quote_mint: Pubkey::new_unique(),
            base_treasury: Pubkey::new_unique(),
            quote_treasury: Pubkey::new_unique(),
            base_decimals: 6,
            quote_decimals: 6,
        };
        let ctx = Context::new(
            chain::rpc("http://127.0.0.1:1"),
            Keypair::new(),
            0,
            addrs,
            MARKETS[0],
            FairValueConfig::default(),
            store.for_market(market, "EURC"),
        );
        let age = Instant::now().duration_since(ctx.last_set_at);
        assert!(
            age >= Duration::from_secs(54) && age < Duration::from_secs(60),
            "clock should start ~55 s in the past, got {age:?}"
        );
        // So the very next pause-path check is already within a tick of the
        // bound, instead of a fresh 60 s away from it.
        let cfg = BotConfig::default();
        assert!(age + cfg.tick > cfg.invalidate.stale_after);
    }

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

    /// EURC is the one market with every tier available, so it exercises the
    /// whole preference order.
    fn eurc() -> MarketConfig {
        *MARKETS
            .iter()
            .find(|m| m.symbol == "EURC")
            .expect("EURC is on the roster")
    }

    /// The tick context the tiering reads, with the engine's real default
    /// staleness bound and a weekday session unless a test says otherwise.
    fn tick_at(now: Instant, now_unix: i64) -> TickCtx {
        TickCtx {
            now,
            now_unix,
            leg_stale: BotConfig::default().fair_value.leg_stale,
            weekend: false,
        }
    }

    /// A hub with one reading in every tier, each at a distinguishable value so
    /// a test can tell which one the cascade picked.
    fn full_hub(now: Instant, now_unix: i64) -> FeedHub {
        let m = eurc();
        let mut hub = FeedHub::new();
        hub.pyth.insert(
            m.currency.to_string(),
            (
                FxQuote {
                    value: 1.1500,
                    confidence: Some(0.0001),
                    publish_time: now_unix,
                },
                now,
            ),
        );
        hub.fx.insert(m.currency.to_string(), (1.1400, now));
        hub.coinbase
            .insert(m.coinbase_product.unwrap().to_string(), (1.1530, now));
        hub.kraken
            .insert(m.kraken_pair.unwrap().to_string(), (1.1520, now));
        hub.kraken
            .insert(USDC_KRAKEN_PAIR.to_string(), (0.9997, now));
        hub.cg
            .insert(m.coingecko_id.unwrap().to_string(), (1.1510, now));
        hub.cg.insert(USDC_COINGECKO_ID.to_string(), (1.0000, now));
        hub.cmc.insert(m.coinmarketcap_id.unwrap(), (1.1490, now));
        hub
    }

    #[test]
    fn every_leg_prefers_its_primary_tier() {
        let (now, now_unix) = (Instant::now(), 1_786_579_250);
        let legs = full_hub(now, now_unix).legs(&eurc(), &tick_at(now, now_unix));
        // Pyth over Frankfurter, and it carries the half-width Frankfurter
        // cannot publish.
        assert_eq!(legs.fx.unwrap().value, 1.1500);
        assert_eq!(legs.fx.unwrap().confidence, Some(0.0001));
        // Coinbase token/USDC over Kraken token/USD over the indices.
        assert_eq!(legs.crypto_usdc.unwrap().value, 1.1530);
        // Kraken's market print over the CoinGecko index.
        assert_eq!(legs.usdc_usd.unwrap().value, 0.9997);
    }

    #[test]
    fn each_leg_falls_through_to_the_next_tier_when_its_primary_is_absent() {
        let (now, now_unix) = (Instant::now(), 1_786_579_250);
        let tick = tick_at(now, now_unix);
        let m = eurc();
        let mut hub = full_hub(now, now_unix);
        hub.pyth.clear();
        hub.coinbase.clear();
        hub.kraken.remove(USDC_KRAKEN_PAIR);
        let legs = hub.legs(&m, &tick);
        assert_eq!(legs.fx.unwrap().value, 1.1400); // Frankfurter
        assert_eq!(legs.fx.unwrap().confidence, None); // and no half-width
        assert_eq!(legs.usdc_usd.unwrap().value, 1.0000); // CoinGecko index
                                                          // Kraken quotes token/USD, so the peg leg converts it to token/USDC —
                                                          // it is not the raw 1.1520 sitting in the cache.
        assert_eq!(legs.crypto_usdc.unwrap().value, 1.1520 / 1.0000);

        // Drop the CEX tier entirely: the indices carry the basis, which is the
        // permanent state for the six markets no CEX lists.
        hub.kraken.clear();
        assert_eq!(hub.legs(&m, &tick).crypto_usdc.unwrap().value, 1.1510);
        hub.cg.remove(m.coingecko_id.unwrap());
        assert_eq!(hub.legs(&m, &tick).crypto_usdc.unwrap().value, 1.1490);
    }

    /// The regression the review caught: a Pyth reading too stale for the
    /// engine must hand the anchor to Frankfurter, not sit in the slot masking
    /// it. The hand-off used to wait on a 24 h ceiling, so a 20-minute Hermes
    /// outage darkened the anchor for the rest of the day while a healthy ECB
    /// rate went unread.
    #[test]
    fn a_stale_pyth_reading_hands_the_anchor_over_instead_of_masking_it() {
        let (now, now_unix) = (Instant::now(), 1_786_579_250);
        let m = eurc();
        let mut hub = full_hub(now, now_unix);
        let (q, t) = hub.pyth[m.currency];
        // 20 minutes old: inside the old 24 h ceiling, past the 15-minute bound.
        hub.pyth.insert(
            m.currency.to_string(),
            (
                FxQuote {
                    publish_time: now_unix - 20 * 60,
                    ..q
                },
                t,
            ),
        );
        let legs = hub.legs(&m, &tick_at(now, now_unix));
        assert_eq!(
            legs.fx.unwrap().value,
            1.1400,
            "Frankfurter should carry it"
        );
    }

    /// The same regression on the two legs that were left tiering on
    /// *presence*. `aged` returns `Some` for a reading of any age and these
    /// caches never evict, so a dead Coinbase used to pin the basis leg for the
    /// life of the process: the tiers beneath it never ran, and the market
    /// degraded on a stale leg instead of falling through to a live print. The
    /// cost was a missed recovery rather than a wrong price, which is why it
    /// survived review — the engine still recognised the leg as stale.
    #[test]
    fn a_stale_basis_or_peg_tier_falls_through_instead_of_masking_the_next_one() {
        let (base, now_unix) = (Instant::now(), 1_786_579_250);
        let m = eurc();
        let mut hub = full_hub(base, now_unix);

        // Read the same hub 20 minutes on — past the engine's 15-minute bound,
        // so every reading `full_hub` seeded is now stale. Ageing the *reader*
        // rather than back-dating the entries keeps this off `Instant`
        // subtraction, which has no monotonic floor to stand on.
        let now = base + Duration::from_secs(20 * 60);
        let tick = tick_at(now, now_unix);

        // With every gated tier stale, each leg lands on its ungated floor
        // instead of holding the stale primary: CMC for the basis, CoinGecko
        // for the peg. Those two are read without a gate on purpose — nothing
        // sits below them to reach.
        let legs = hub.legs(&m, &tick);
        assert_eq!(legs.usdc_usd.unwrap().value, 1.0000, "CoinGecko peg floor");
        assert_eq!(legs.crypto_usdc.unwrap().value, 1.1490, "CMC basis floor");

        // Now refresh only Kraken. It outranks both floors again on both legs —
        // the recovery the presence check could never reach, because the stale
        // Coinbase and Kraken readings above simply masked them.
        hub.kraken
            .insert(m.kraken_pair.unwrap().to_string(), (1.1520, now));
        hub.kraken.insert(USDC_KRAKEN_PAIR.to_string(), (0.9997, now));
        let legs = hub.legs(&m, &tick);
        assert_eq!(legs.usdc_usd.unwrap().value, 0.9997);
        // Kraken quotes token/USD, so the peg converts it, as on the live path.
        assert_eq!(legs.crypto_usdc.unwrap().value, 1.1520 / 0.9997);
    }

    /// …but not while the FX session is shut. Frankfurter is aged from receipt,
    /// so it reads fresh all weekend off a Friday close; standing it up would
    /// hold the engine in the Normal regime on a closed market instead of
    /// flipping the anchor to the crypto reference (§1 fm2).
    #[test]
    fn the_fx_fallback_is_suppressed_while_the_session_is_closed() {
        let (now, now_unix) = (Instant::now(), 1_786_579_250);
        let m = eurc();
        let mut hub = full_hub(now, now_unix);
        hub.pyth.clear();
        let mut tick = tick_at(now, now_unix);
        tick.weekend = true;
        assert!(hub.legs(&m, &tick).fx.is_none());
        // The basis leg is untouched by the session — it is what anchors the
        // crypto-only regime.
        assert!(hub.legs(&m, &tick).crypto_usdc.is_some());
    }

    /// A market with no CEX listing must never pick up another market's pair
    /// out of the shared caches — the tier is keyed by *this* market's config.
    #[test]
    fn a_market_with_no_cex_listing_uses_the_index_tier() {
        let (now, now_unix) = (Instant::now(), 1_786_579_250);
        let mut hub = full_hub(now, now_unix);
        let zarp = *MARKETS.iter().find(|m| m.symbol == "ZARP").unwrap();
        hub.cg
            .insert(zarp.coingecko_id.unwrap().to_string(), (0.0605, now));
        hub.pyth.insert(
            zarp.currency.to_string(),
            (
                FxQuote {
                    value: 0.0600,
                    confidence: None,
                    publish_time: now_unix,
                },
                now,
            ),
        );
        let legs = hub.legs(&zarp, &tick_at(now, now_unix));
        assert!(zarp.coinbase_product.is_none() && zarp.kraken_pair.is_none());
        assert_eq!(legs.crypto_usdc.unwrap().value, 0.0605);
    }

    #[test]
    fn a_pyth_reading_ages_from_its_publish_time_not_from_receipt() {
        // The weekend case: the poller keeps answering (receipt age ~0) with a
        // rate published two hours ago. Ageing from receipt would show this as
        // fresh forever and the crypto-only regime would never engage.
        let now = Instant::now();
        let q = FxQuote {
            value: 1.15,
            confidence: Some(0.0001),
            publish_time: 1_786_579_250,
        };
        let r = pyth_reading(&q, now, now, 1_786_579_250 + 7_200);
        assert_eq!(r.age, Duration::from_secs(7_200));
        assert!(!r.fresh(Duration::from_secs(15 * 60)));
    }

    #[test]
    fn a_dead_poller_still_ages_a_recently_published_reading() {
        // The converse: `publish_time` looks current but nothing has been
        // received in an hour, so the receipt age floors the result.
        let now = Instant::now();
        let read_at = now - Duration::from_secs(3_600);
        let q = FxQuote {
            value: 1.15,
            confidence: Some(0.0001),
            publish_time: 1_786_579_250,
        };
        let r = pyth_reading(&q, read_at, now, 1_786_579_250);
        assert!(r.age >= Duration::from_secs(3_600));
    }

    #[test]
    fn a_skewed_clock_degrades_to_stale_rather_than_wrapping() {
        // A `publish_time` in the future must not wrap into a huge age, and a
        // nonsensical one must not read as fresh either.
        let now = Instant::now();
        let q = FxQuote {
            value: 1.15,
            confidence: None,
            publish_time: 1_786_579_250,
        };
        // Ordinary sub-minute skew is tolerated: age floors at the receipt age.
        let skewed = pyth_reading(&q, now, now, 1_786_579_250 - 30);
        assert_eq!(skewed.age, Duration::ZERO);
        // Clock absurdly ahead: clamped, and the cascade drops it for the
        // Frankfurter tier rather than quoting off it.
        let stale = pyth_reading(&q, now, now, 1_786_579_250 + 10_000_000);
        assert_eq!(stale.age, MAX_PYTH_AGE);
    }

    /// The security lens's finding: `publish_time` is venue-supplied data used
    /// as a clock, so a stamp far in the *future* must not pin the age at zero.
    /// Left unbounded, a frozen rate re-served with a forward-dated stamp reads
    /// as perpetually fresh — exactly what publish-time ageing exists to stop.
    #[test]
    fn a_far_future_publish_time_is_bogus_rather_than_freshest_possible() {
        let now = Instant::now();
        let q = FxQuote {
            value: 1.15,
            confidence: Some(0.0001),
            publish_time: 1_786_579_250,
        };
        for ahead in [3_600i64, 86_400, 31_536_000] {
            let r = pyth_reading(&q, now, now, 1_786_579_250 - ahead);
            assert_eq!(r.age, MAX_PYTH_AGE, "{ahead}s ahead should read as bogus");
            assert!(!r.fresh(Duration::from_secs(15 * 60)));
        }
    }

    #[test]
    fn an_absurdly_aged_pyth_reading_hands_the_anchor_to_frankfurter() {
        let now = Instant::now();
        let now_unix = 1_786_579_250;
        let mut hub = full_hub(now, now_unix);
        // Re-stamp the Pyth reading as published long ago.
        let m = eurc();
        let (q, t) = hub.pyth[m.currency];
        hub.pyth.insert(
            m.currency.to_string(),
            (
                FxQuote {
                    publish_time: now_unix - 10_000_000,
                    ..q
                },
                t,
            ),
        );
        assert_eq!(
            hub.legs(&m, &tick_at(now, now_unix)).fx.unwrap().value,
            1.1400
        );
    }

    #[test]
    fn drain_entries_into_caches_the_latest_per_product() {
        let (tx, mut rx) = broadcast::channel::<(String, f64)>(8);
        tx.send(("EURC-USDC".to_string(), 1.1520)).unwrap();
        tx.send(("EURC-USDC".to_string(), 1.1530)).unwrap();
        let mut cache: HashMap<String, (f64, Instant)> = HashMap::new();
        let now = Instant::now();
        drain_entries_into(&mut rx, &mut cache, now);
        assert_eq!(cache["EURC-USDC"].0, 1.1530);
        // Drained dry — a second pass is a no-op, not a re-read.
        drain_entries_into(&mut rx, &mut cache, now);
        assert_eq!(cache.len(), 1);
    }
}
