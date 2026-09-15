//! The fair-value estimator process: read the store, compose one fair value per
//! market, publish the tick.
//!
//! This is the driver the [`crate::fair_price`] module said "does not exist yet".
//! That module serializes one finished composition and nothing else; this one
//! reads the legs, holds the stateful engines, decides when a tick happens, and
//! owns the failure posture. The split is the same one the store side keeps:
//! producing a composition and writing it down are different jobs.
//!
//! # The per-market three-leg mapping
//!
//! Nothing in the repo held this mapping before, and this module is its one
//! home. The engine composes `fair = fx × basis` from three candidate legs plus
//! a static fallback (`docs/market-making.md` §1); a *published product* has to
//! be resolved onto the store series that feed each of them:
//!
//! | Leg           | Series                        | Table         |
//! | ------------- | ----------------------------- | ------------- |
//! | `fx`          | `<CCY>-USD` per FX venue      | `cex_prices`  |
//! | `crypto_usdc` | the market's own `*-USDC` pair | `cex_prices`  |
//! | `usdc_usd`    | `USDC-USD`, one series        | `spot_ticks`  |
//! | `static_usd`  | a per-market constant         | — (config)    |
//!
//! **The peg leg is in the other table, and that is why this crate has two
//! readers.** `USDC-USD` is written by one collector, `market-data-kraken`, into
//! `spot_ticks`; nothing writes it to `cex_prices`. See
//! [`crate::tick_store`] for the full writer split and why a `UNION` would be
//! the wrong shape.
//!
//! **The peg leg is portfolio-wide, not per-market.** A USDC/USD deviation is
//! one event for the whole book (§1 fm1), so one resolved reading is offered to
//! every market rather than each market reading its own copy.
//!
//! # One engine per published product
//!
//! Migration 0011 is explicit that the engine carries a **per-market basis
//! EMA**, so this holds one [`FairValueEngine`] per published product key.
//! Sharing one would fork the basis history across markets — a `Clone` of an
//! engine is a fork of its accumulator, which is why the type is deliberately
//! not `Copy`.
//!
//! # Two markets are pinned, and that is the operator's re-scope
//!
//! Only EURC composes on an **observed** basis. AUDD and CADC quote off the FX
//! anchor times a pinned 1:1 redemption peg, because the re-scope for the first
//! fills says the FX composite alone is enough for them — and because a thin
//! product's ticker returns its last print whether or not one happened recently,
//! so an unwired basis is honest where a stale-print basis would *corroborate*
//! the leg with a number nobody traded. The engine drops a pinned market's whole
//! crypto candidate set unconditionally, so a stray reading can never price one.
//!
//! # Failure posture: this process classifies its own errors
//!
//! **It does not run on the feeds runner, and that is the point.** The runner
//! sleeps its backoff and continues on *any* source error, forever — a policy
//! that is right for a collector (a missed poll is a gap in a series) and wrong
//! for a publisher. Blind-retrying a permanent publish failure is precisely the
//! silent stall [`PublishError`]'s split exists to prevent: the process would
//! look busy, publish nothing, and report a rising retry count instead of a
//! defect. So the tick loop lives here and branches on
//! [`PublishError::retryable`].
//!
//! Three halts, each naming its cause in the wire vocabulary
//! [`PublishError::class`] already owns — see [`Halt`]:
//!
//! * a **permanent** publish failure halts on the first occurrence;
//! * **transient** publish failures halt once they have persisted past
//!   [`MAX_PUBLISH_RETRY_WINDOW`], because "retry forever" is the same stall in
//!   slow motion;
//! * an unreadable store halts past [`crate::fx_store::MAX_STORE_SILENCE`],
//!   since a composition resting on nothing is worse than no composition.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use dropset_fair_value::{
    Candidates, ClockCtx, FairValue, FairValueConfig, FairValueEngine, LegStaleness, Legs,
};
use dropset_feeds::now_secs;
use sqlx::PgPool;

use crate::fair_price::{publish, PublishError};
use crate::fx_store::{
    fx_product_id, push_store_candidate, store_reading, store_unavailable, FxStoreRow,
    FxStoreSource, FX_STORE_SOURCES,
};
use crate::tick_store::{SpotTickRow, SpotTickSource, SOURCE_KRAKEN, USDC_USD_PRODUCT};

/// `cex_prices.source` for the crypto reference venue.
///
/// Matches `market-data/src/bin/coinbase.rs`'s own `SOURCE` literally — a join
/// by string, so the two sides have to move together. The **candle** collector,
/// not the ticker: the ticker writes `spot_ticks` and republishes a last print
/// whether or not one happened, which is the aging problem the pinned markets
/// exist to avoid.
pub const SOURCE_COINBASE: &str = "coinbase";

/// How long publishing may keep failing **transiently** before the estimator
/// halts anyway.
///
/// Sized like [`crate::fx_store::MAX_STORE_SILENCE`] and for the same reason: a
/// rolling restart, a failover or a burst of contention must not halt a
/// publisher, while a genuine outage must stop it *visibly* rather than retrying
/// into the void. A retryable failure that has not cleared in five minutes is no
/// longer an operational blip.
///
/// **A window rather than a retry count**, so the bound does not silently change
/// meaning when [`EstimatorConfig::tick_interval`] does. A count of ten means
/// fifty seconds at a 5s tick and five minutes at a 30s one; the thing worth
/// bounding is elapsed time, so that is what is bounded.
pub const MAX_PUBLISH_RETRY_WINDOW: Duration = Duration::from_secs(5 * 60);

/// One published market, resolved onto the store series that feed its legs.
///
/// `&'static str` throughout rather than `String`: this is a compiled-in roster,
/// and the engine's candidate API takes `&'static str` source labels anyway so a
/// contributor name can be reported without an allocation per tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EstimatorMarket {
    /// The canonical product published to `fair_price`, e.g. `EURC-USDC`.
    ///
    /// Must satisfy 0011's `^[A-Z0-9]{2,10}-[A-Z0-9]{2,10}$` CHECK — a
    /// violation is a [`PublishError::Permanent`], so it is pinned by a test
    /// here rather than discovered as a halt in production.
    pub product_id: &'static str,
    /// The ISO 4217 currency the market tracks, e.g. `EUR`. The FX leg's store
    /// series is derived from it by [`fx_product_id`], so the canonical spelling
    /// is computed rather than duplicated.
    pub currency: &'static str,
    /// The market's own crypto reference pair in `cex_prices`, or `None` for a
    /// market composing on a pinned basis.
    ///
    /// Exactly one of this and [`Self::pinned_basis`] is `Some` — see the test
    /// that enforces it, and the module docs for why two markets are pinned.
    pub crypto_product: Option<&'static str>,
    /// Basis to pin because the market has no independent basis source.
    pub pinned_basis: Option<f64>,
    /// Last-resort static USD-per-token peg, used only when every live leg is
    /// down. A representative spot value; a live anchor supersedes it whenever
    /// the feeds answer.
    pub static_usd: f64,
}

impl EstimatorMarket {
    /// The canonical `cex_prices` series for this market's FX anchor leg.
    ///
    /// `None` only for a USD-tracking market, which has no cross — a case the
    /// roster does not currently contain and which the invariant test pins as
    /// absent rather than leaving to `unwrap`.
    pub fn fx_product(&self) -> Option<String> {
        fx_product_id(self.currency)
    }

    /// This market's calibration, which is the default plus its own basis pin.
    ///
    /// Every other constant is a marked TBD(analytics) placeholder shared across
    /// markets. Reading the pin from the roster rather than from an environment
    /// variable is deliberate: it is a statement about which *sources exist*,
    /// not a tuning knob, so it belongs beside the leg mapping it follows from.
    pub fn config(&self) -> FairValueConfig {
        FairValueConfig {
            pinned_basis: self.pinned_basis,
            ..FairValueConfig::default()
        }
    }
}

/// The published roster — the three MVP pairs, at ~$100 top-of-book.
///
/// Deliberately **not** derived from the maker bot's own `MARKETS`: this crate
/// cannot depend on that one (the dependency runs the other way, since the maker
/// reads [`crate::fx_store`]), and the two rosters answer different questions
/// anyway — the maker's carries mint keypairs and quoting parameters, and this
/// one carries store series. The overlap that *matters* is the basis pin, so the
/// pins here are pinned against the maker's by
/// `market-data/tests/estimator_roster_agreement.rs`.
pub const MVP_MARKETS: [EstimatorMarket; 3] = [
    EstimatorMarket {
        product_id: "EURC-USDC",
        currency: "EUR",
        // The one market with a wired CEX basis: Coinbase lists `EURC-USDC` and
        // the candle collector rosters it.
        crypto_product: Some("EURC-USDC"),
        pinned_basis: None,
        static_usd: 1.14,
    },
    EstimatorMarket {
        product_id: "AUDD-USDC",
        currency: "AUD",
        crypto_product: None,
        pinned_basis: Some(1.0),
        static_usd: 0.7214,
    },
    EstimatorMarket {
        product_id: "CADC-USDC",
        currency: "CAD",
        crypto_product: None,
        pinned_basis: Some(1.0),
        static_usd: 0.7244,
    },
];

/// Why the estimator stopped.
///
/// Every variant is fail-closed: the process exits and its orchestrator restarts
/// it, which is the visible outcome. The alternative each one refuses is a
/// process that keeps ticking while publishing nothing.
///
/// **The publish class is carried rather than re-encoded.** A second enum
/// mapping "permanent" and "transient" to words would be a second vocabulary for
/// one fact, free to drift from [`PublishError::class`]; this holds that
/// function's own output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Halt {
    /// A publish failed and retrying will not help, or has not helped for
    /// [`MAX_PUBLISH_RETRY_WINDOW`]. `class` is [`PublishError::class`].
    Publish { class: &'static str },
    /// The store could not be read for [`crate::fx_store::MAX_STORE_SILENCE`].
    /// The legs rest on nothing, so composing would be inventing a price.
    StoreSilent,
}

impl Halt {
    /// The wire-stable reason, for the log line and the operator alert that key
    /// off it.
    pub fn reason(self) -> String {
        match self {
            Self::Publish { class } => format!("publish:{class}"),
            Self::StoreSilent => "store_silent".to_string(),
        }
    }
}

impl std::fmt::Display for Halt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Publish { class } => {
                write!(f, "{class} publish failure; estimator halting")
            }
            Self::StoreSilent => {
                f.write_str("the market-data store is unreadable; estimator halting")
            }
        }
    }
}

impl std::error::Error for Halt {}

/// Environment-driven configuration. `DATABASE_URL` is required; the tick
/// cadence has a default sized to the collectors'.
#[derive(Clone, Debug)]
pub struct EstimatorConfig {
    pub database_url: String,
    /// Seconds between composed-and-published ticks.
    pub tick_interval: Duration,
}

/// Seconds between estimator ticks.
///
/// Matched to the candle collectors' own poll cadence: the finest bucket any
/// venue publishes is 60s, so ticking faster than the collectors poll composes
/// repeatedly from the same rows and writes a row per tick to say so.
///
/// **Floored at one second, and that floor is load-bearing.** `fair_price`'s
/// primary key is `(product_id, ts)` in whole seconds, so two ticks inside one
/// second collide — the second one lands on the key, writes nothing, and
/// [`publish`] reports `false`, which is defined as an alarm meaning "something
/// else published for this pair". A sub-second cadence would raise that alarm
/// against itself.
const DEFAULT_TICK_INTERVAL_SECS: u64 = 15;

impl EstimatorConfig {
    pub fn from_env() -> Result<Self> {
        let database_url =
            std::env::var("DATABASE_URL").map_err(|_| anyhow!("DATABASE_URL is required"))?;
        Ok(Self {
            database_url,
            tick_interval: tick_interval_from_secs(
                std::env::var("TICK_INTERVAL_SECS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok()),
            ),
        })
    }
}

/// The tick cadence for a configured `secs`, or the default when unset.
///
/// A named function rather than a chain inside [`EstimatorConfig::from_env`] so
/// the **clamp** is testable without setting a process-wide environment variable
/// — which is what a test of `from_env` would have to do, and which races every
/// other test in the binary. The clamp is the part worth pinning: see
/// [`DEFAULT_TICK_INTERVAL_SECS`] for why a zero collides with itself on
/// `fair_price`'s primary key.
fn tick_interval_from_secs(secs: Option<u64>) -> Duration {
    Duration::from_secs(secs.unwrap_or(DEFAULT_TICK_INTERVAL_SECS).max(1))
}

/// One tick's worth of store rows, and how long ago they were read.
///
/// The read instant is carried because a *failed* read reuses the previous
/// snapshot: the rows' own stamps stop moving, so the receipt floor is the only
/// thing that can age the legs out. This is the shape
/// [`crate::fx_store::store_reading_age`]'s floor exists for.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    candles: Vec<FxStoreRow>,
    ticks: Vec<SpotTickRow>,
    read_at: Option<Instant>,
}

impl Snapshot {
    /// How long ago these rows were read, or zero before the first read.
    fn receipt_age(&self, now: Instant) -> Duration {
        self.read_at
            .map(|at| now.saturating_duration_since(at))
            .unwrap_or_default()
    }
}

/// The estimator process.
pub struct Estimator {
    pool: PgPool,
    markets: Vec<EstimatorMarket>,
    /// One engine per published product key — see the module docs.
    engines: HashMap<&'static str, FairValueEngine>,
    candles: FxStoreSource,
    ticks: SpotTickSource,
    tick_interval: Duration,
}

impl Estimator {
    /// Wire an estimator for `markets` against `pool`.
    ///
    /// Both readers are constructed here from the roster, so the series asked
    /// for and the series composed from cannot diverge: a market added to the
    /// roster widens the queries by construction rather than by a second edit.
    pub fn new(
        pool: PgPool,
        markets: Vec<EstimatorMarket>,
        tick_interval: Duration,
    ) -> Result<Self> {
        if markets.is_empty() {
            return Err(anyhow!(
                "no markets to estimate; the roster resolved to nothing"
            ));
        }

        let mut engines = HashMap::with_capacity(markets.len());
        let mut candle_products = Vec::new();
        for m in &markets {
            let fx = m.fx_product().ok_or_else(|| {
                anyhow!(
                    "{} tracks {} which has no FX cross; it cannot be composed",
                    m.product_id,
                    m.currency
                )
            })?;
            candle_products.push(fx);
            if let Some(crypto) = m.crypto_product {
                candle_products.push(crypto.to_string());
            }
            if engines
                .insert(m.product_id, FairValueEngine::new(m.config()))
                .is_some()
            {
                return Err(anyhow!(
                    "{} appears twice in the roster; its basis history would fork",
                    m.product_id
                ));
            }
        }

        // The FX venues plus the crypto reference venue. One reader over the
        // union rather than two over the same table: the statement filters
        // `source = ANY($1) AND product_id = ANY($2)`, so asking an FX venue for
        // a `*-USDC` pair simply returns no rows.
        let mut candle_sources: Vec<String> =
            FX_STORE_SOURCES.iter().map(|s| s.to_string()).collect();
        candle_sources.push(SOURCE_COINBASE.to_string());

        Ok(Self {
            candles: FxStoreSource::new(
                "estimator:cex_prices",
                pool.clone(),
                candle_sources,
                candle_products,
            ),
            ticks: SpotTickSource::peg("estimator:spot_ticks", pool.clone()),
            pool,
            markets,
            engines,
            tick_interval,
        })
    }

    /// Compose and publish until a [`Halt`] fires or a shutdown signal arrives.
    pub async fn run(mut self) -> Result<()> {
        let mut snapshot = Snapshot::default();
        let started = Instant::now();
        let mut last_read_ok: Option<Instant> = None;
        let mut publish_failing_since: Option<Instant> = None;
        let mut last_tick: Option<Instant> = None;

        tracing::info!(
            markets = self.markets.len(),
            tick_secs = self.tick_interval.as_secs(),
            "fair-value estimator starting"
        );

        loop {
            let now = Instant::now();

            // --- read ------------------------------------------------------
            match self.read().await {
                Ok(fresh) => {
                    snapshot = fresh;
                    last_read_ok = Some(now);
                }
                Err(err) => {
                    // A failed read is not a publish failure and must not borrow
                    // that vocabulary. The previous rows stay in play and age
                    // out on the receipt floor; silence past the bound halts.
                    let silent_for = last_read_ok
                        .map(|at| now.saturating_duration_since(at))
                        // Before the first successful read, silence is measured
                        // from startup — otherwise a store that is down at boot
                        // never answers, so nothing ever elapses and the guard
                        // stays suppressed forever.
                        .unwrap_or_else(|| now.saturating_duration_since(started));
                    if store_unavailable(silent_for) {
                        let halt = Halt::StoreSilent;
                        tracing::error!(
                            reason = %halt.reason(),
                            silent_secs = silent_for.as_secs(),
                            error = %err,
                            "{halt}"
                        );
                        return Err(anyhow::Error::new(halt).context(format!("{err:#}")));
                    }
                    tracing::warn!(
                        silent_secs = silent_for.as_secs(),
                        error = %err,
                        "store read failed; composing from the cached snapshot"
                    );
                }
            }

            // --- compose and publish --------------------------------------
            let dt = last_tick
                .map(|at| now.saturating_duration_since(at))
                .unwrap_or(self.tick_interval);
            last_tick = Some(now);

            match self.publish_tick(&snapshot, now, dt).await {
                Ok(()) => publish_failing_since = None,
                Err(err) => {
                    let class = err.class();
                    // Permanent: the row is refused on the schema's own terms, so
                    // every retry is a spin. Halt on the first one.
                    if !err.retryable() {
                        let halt = Halt::Publish { class };
                        tracing::error!(reason = %halt.reason(), error = ?err, "{halt}");
                        return Err(anyhow::Error::new(halt).context(format!("{err:?}")));
                    }
                    // Transient: retry, but not forever — see
                    // MAX_PUBLISH_RETRY_WINDOW.
                    let since = *publish_failing_since.get_or_insert(now);
                    let failing_for = now.saturating_duration_since(since);
                    if failing_for > MAX_PUBLISH_RETRY_WINDOW {
                        let halt = Halt::Publish { class };
                        tracing::error!(
                            reason = %halt.reason(),
                            failing_secs = failing_for.as_secs(),
                            error = ?err,
                            "{halt}"
                        );
                        return Err(anyhow::Error::new(halt).context(format!("{err:?}")));
                    }
                    tracing::warn!(
                        class,
                        failing_secs = failing_for.as_secs(),
                        error = ?err,
                        "publish failed; retrying on the next tick"
                    );
                }
            }

            // --- wait ------------------------------------------------------
            tokio::select! {
                _ = tokio::time::sleep(self.tick_interval) => {}
                _ = shutdown() => {
                    tracing::info!("shutdown signal received; estimator stopping");
                    return Ok(());
                }
            }
        }
    }

    /// Read both tables for one tick.
    ///
    /// Either read failing fails the whole snapshot, deliberately: the peg leg
    /// is in the other table, and a half-read tick would compose every market
    /// with a silently absent common-mode guard while reporting a successful
    /// read.
    async fn read(&self) -> Result<Snapshot> {
        let candles = self
            .candles
            .latest()
            .await
            .context("reading cex_prices for the FX and crypto legs")?;
        let ticks = self
            .ticks
            .latest()
            .await
            .context("reading spot_ticks for the USDC/USD peg leg")?;
        Ok(Snapshot {
            candles,
            ticks,
            read_at: Some(Instant::now()),
        })
    }

    /// Compose every market and publish the whole tick in one transaction.
    ///
    /// **One transaction, so the tick is atomic.** With a bare pool each market
    /// is its own auto-commit round trip, so an interrupted tick leaves some
    /// pairs written at `ts` and others absent — indistinguishable, to a reader,
    /// from pairs the estimator skipped.
    async fn publish_tick(
        &mut self,
        snapshot: &Snapshot,
        now: Instant,
        dt: Duration,
    ) -> Result<(), PublishError> {
        let ts = now_secs();
        let clock = ClockCtx::from_unix(ts as u64);
        let receipt_age = snapshot.receipt_age(now);

        // The peg leg is portfolio-wide, so it is resolved once and offered to
        // every market rather than rebuilt per market.
        let peg = peg_candidates(&snapshot.ticks, ts, receipt_age);

        let mut tx = self.pool.begin().await?;
        for market in &self.markets {
            let engine = self
                .engines
                .get_mut(market.product_id)
                // Unreachable: `new` builds one engine per roster entry and
                // refuses a duplicate. Not an `expect`, because a panic inside
                // the publish path would destroy the halt this path exists to
                // report.
                .ok_or_else(|| {
                    PublishError::Permanent(anyhow!(
                        "{} has no engine; the roster and the engine map disagree",
                        market.product_id
                    ))
                })?;

            // `Candidates` is `Copy`, so every market gets the same resolved peg
            // leg by value — which is the intent: one portfolio-wide reading.
            let legs = build_legs(market, snapshot, ts, receipt_age, peg);
            let (stale, _dispersion) = engine.leg_bounds();
            let fair = engine.compose(legs, dt, clock);

            // The bounds the composition was ACTUALLY resolved at, taken from the
            // engine that resolved it rather than from a second copy of the
            // config — 0014 records them per row, and a bound looked up
            // elsewhere could disagree with the one the engine used.
            let fresh = publish(&mut *tx, ts, market.product_id, &fair, stale).await?;
            log_tick(market, &fair, stale, fresh);
        }
        // The commit completes the publish, so it is classified on the same
        // terms — a deferred constraint is permanent, a serialization failure is
        // not. See `PublishError`'s `From<sqlx::Error>`.
        tx.commit().await?;
        Ok(())
    }
}

/// Assemble one market's [`Legs`] from a snapshot.
///
/// A free function rather than a method so it is testable without a pool: this
/// is the mapping the module exists to own, so a test should be able to pin it
/// without a database.
fn build_legs(
    market: &EstimatorMarket,
    snapshot: &Snapshot,
    now_unix: i64,
    receipt_age: Duration,
    usdc_usd: Candidates,
) -> Legs {
    Legs {
        fx: fx_candidates(market, &snapshot.candles, now_unix, receipt_age),
        crypto_usdc: crypto_candidates(market, &snapshot.candles, now_unix, receipt_age),
        usdc_usd,
        static_usd: market.static_usd,
    }
}

/// The FX anchor leg: every rostered FX venue that has a row for this market's
/// cross, at its ruled designation.
///
/// **Iterates [`FX_STORE_SOURCES`] rather than the rows**, for two reasons that
/// both matter. The offer order is load-bearing — that constant *is* an offer
/// chain, and a reference offered before a tape would be the candidate an
/// over-full leg keeps — and it yields `&'static str` labels, which is what the
/// candidate API needs to report a contributor without allocating per tick.
fn fx_candidates(
    market: &EstimatorMarket,
    rows: &[FxStoreRow],
    now_unix: i64,
    receipt_age: Duration,
) -> Candidates {
    let Some(product) = market.fx_product() else {
        return Candidates::none();
    };
    let mut candidates = Candidates::none();
    for source in FX_STORE_SOURCES {
        let reading = rows
            .iter()
            .find(|r| r.source == source && r.product_id == product)
            .and_then(|r| store_reading(r.close, r.published_at, now_unix, receipt_age));
        candidates = push_store_candidate(candidates, source, reading);
    }
    candidates
}

/// The crypto reference leg: the market's own `*-USDC` candle series.
///
/// Offered as an **untrusted** tape. One venue is all this leg has, so the
/// engine resolves it as an explicit single-source state and the market composes
/// as [`dropset_fair_value::Regime::Uncorroborated`] — quoted, and no longer
/// described as corroborated. Designating it believable-alone would claim
/// corroboration that does not exist.
///
/// Empty for a pinned market, which has no such series. The engine drops a
/// pinned market's crypto set unconditionally anyway; offering nothing here
/// means the two agree rather than relying on that.
fn crypto_candidates(
    market: &EstimatorMarket,
    rows: &[FxStoreRow],
    now_unix: i64,
    receipt_age: Duration,
) -> Candidates {
    let Some(product) = market.crypto_product else {
        return Candidates::none();
    };
    let reading = rows
        .iter()
        .find(|r| r.source == SOURCE_COINBASE && r.product_id == product)
        .and_then(|r| store_reading(r.close, r.published_at, now_unix, receipt_age));
    Candidates::none().push(SOURCE_COINBASE, reading)
}

/// The portfolio-wide USDC/USD peg leg, from `spot_ticks`.
///
/// Offered as an untrusted tape: it is a real market print rather than an issuer
/// redemption rate, and one venue publishes it. An empty leg means the
/// common-mode guard cannot fire this tick — which the engine treats as exactly
/// that, not as a fault.
fn peg_candidates(rows: &[SpotTickRow], now_unix: i64, receipt_age: Duration) -> Candidates {
    let reading = rows
        .iter()
        .find(|r| r.source == SOURCE_KRAKEN && r.product_id == USDC_USD_PRODUCT)
        .and_then(|r| r.reading(now_unix, receipt_age));
    Candidates::none().push(SOURCE_KRAKEN, reading)
}

/// One line per market per tick.
///
/// `fresh == false` is logged at **warn**: [`publish`] defines it as the primary
/// key already holding this pair at this stamp, which means something else
/// published for it. A silent `false` would hide a second estimator writing the
/// same series.
fn log_tick(market: &EstimatorMarket, fair: &FairValue, stale: LegStaleness, fresh: bool) {
    if !fresh {
        tracing::warn!(
            product_id = market.product_id,
            "a row already existed for this pair at this stamp; another publisher is writing it"
        );
    }
    tracing::info!(
        product_id = market.product_id,
        fair = ?fair.fair,
        regime = ?fair.regime,
        health = ?fair.health,
        basis = ?fair.basis,
        fx_sources = fair.fx_leg.n,
        crypto_sources = fair.crypto_leg.n,
        tape_bound_secs = stale.tape.as_secs(),
        "composed"
    );
}

/// Resolve when the process should stop: `SIGTERM` (what an orchestrator sends)
/// or `SIGINT`.
///
/// Both, because handling only `ctrl_c` means a container stop falls through to
/// the runtime's `SIGKILL` grace period — the tick in flight is abandoned rather
/// than finished, and the publish it was mid-transaction on rolls back with
/// nothing said about it.
async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            // Registration can only fail on a broken runtime; falling back to
            // SIGINT alone is better than refusing to run.
            Err(err) => {
                tracing::warn!(error = %err, "cannot listen for SIGTERM; SIGINT only");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dropset_fair_value::{Anchor, Regime};

    fn candle(source: &str, product_id: &str, published_at: i64, close: f64) -> FxStoreRow {
        FxStoreRow {
            source: source.to_string(),
            product_id: product_id.to_string(),
            published_at,
            close,
        }
    }

    /// Staleness bounds for a test that is not exercising staleness. Wide
    /// enough that every fixture row below counts, so a leg resolving to
    /// nothing means the mapping is wrong rather than the row being stale.
    fn bounds() -> LegStaleness {
        LegStaleness::uniform(Duration::from_secs(15 * 60))
    }

    fn eurc() -> EstimatorMarket {
        MVP_MARKETS[0]
    }

    fn audd() -> EstimatorMarket {
        MVP_MARKETS[1]
    }

    /// Exactly one of `crypto_product` and `pinned_basis` is set, for every
    /// market.
    ///
    /// The engine drops a pinned market's crypto set unconditionally, so a
    /// market carrying both would silently have its configured source ignored —
    /// a wiring gap that looks like a working configuration. The maker bot
    /// asserts the same invariant over its own roster.
    #[test]
    fn a_market_pins_its_basis_exactly_when_it_has_no_crypto_leg() {
        for m in MVP_MARKETS {
            assert_eq!(
                m.crypto_product.is_none(),
                m.pinned_basis.is_some(),
                "{} must pin its basis exactly when it has no crypto series",
                m.product_id
            );
        }
    }

    /// Every published id satisfies 0011's canonical-shape CHECK.
    ///
    /// A violation is a `PublishError::Permanent` — it halts the estimator on
    /// its first tick, in production, having passed every other check in the
    /// repo. The regex is 0011's, restated here because a runtime-typed insert
    /// gives nothing else a chance to see it.
    #[test]
    fn every_published_id_is_canonical() {
        for m in MVP_MARKETS {
            let (base, quote) = m
                .product_id
                .split_once('-')
                .unwrap_or_else(|| panic!("{} is not BASE-QUOTE", m.product_id));
            for part in [base, quote] {
                assert!(
                    (2..=10).contains(&part.len())
                        && part
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
                    "{} violates 0011's canonical-shape CHECK",
                    m.product_id
                );
            }
        }
    }

    /// Every market resolves an FX cross, and none is USD.
    ///
    /// `fx_product` returns `None` for USD, which `Estimator::new` refuses — so
    /// this pins the roster as constructible rather than leaving the refusal to
    /// a startup failure.
    #[test]
    fn every_market_resolves_an_fx_cross() {
        for m in MVP_MARKETS {
            assert_eq!(
                m.fx_product().as_deref(),
                Some(format!("{}-USD", m.currency).as_str()),
                "{} must derive its FX series from its currency",
                m.product_id
            );
        }
    }

    /// No two markets share a published id, which would fork a basis EMA.
    #[test]
    fn published_ids_are_unique() {
        let mut ids: Vec<&str> = MVP_MARKETS.iter().map(|m| m.product_id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "a published id appears twice");
    }

    /// The FX leg offers each rostered venue at its **ruled designation** — the
    /// two tapes into the fast set, the daily fix as a reference behind them.
    ///
    /// Asserted through the two designations rather than through a count of
    /// offers, because a count cannot tell them apart: `Consensus::n` is a
    /// statement about the *fast* set, so all three venues answering reads as
    /// `n == 2`. A test asserting three would be asserting that Alpha Vantage
    /// pools into the live median, which is exactly what its reference class
    /// exists to prevent.
    #[test]
    fn the_fx_leg_offers_every_venue_at_its_designation() {
        let all_three = vec![
            candle("oanda", "EUR-USD", 1_000, 1.1400),
            candle("twelvedata", "EUR-USD", 1_000, 1.1402),
            candle("alphavantage", "EUR-USD", 1_000, 1.1390),
            // A different pair, which must not be picked up.
            candle("oanda", "AUD-USD", 1_000, 0.72),
        ];
        let resolved = fx_candidates(&eurc(), &all_three, 1_060, Duration::from_secs(1))
            .resolve(bounds(), 0.01);
        assert_eq!(
            resolved.n, 2,
            "both tapes corroborate the fast signal; the daily fix must not join it"
        );
        assert!(resolved.reading.is_some());

        // With no tape present the reference carries the leg — the fallback that
        // keeps a fix-only market from darking. This is what proves Alpha
        // Vantage was offered at all rather than dropped.
        let fix_only = vec![candle("alphavantage", "EUR-USD", 1_000, 1.1390)];
        let resolved = fx_candidates(&eurc(), &fix_only, 1_060, Duration::from_secs(1))
            .resolve(bounds(), 0.01);
        assert_eq!(
            resolved.reading.map(|r| r.value),
            Some(1.1390),
            "a reference must be able to carry the leg alone"
        );
    }

    /// A market's crypto leg reads its own pair from the candle venue only.
    ///
    /// The ticker writes `spot_ticks` under the same `coinbase` label, so a
    /// reader that matched on the source alone would be reading a different
    /// table's convention. This pins that the pair has to match too.
    #[test]
    fn the_crypto_leg_reads_only_its_own_pair() {
        let rows = vec![
            candle(SOURCE_COINBASE, "EURC-USDC", 1_000, 1.1410),
            candle(SOURCE_COINBASE, "AUDD-USDC", 1_000, 0.7220),
        ];
        let resolved = crypto_candidates(&eurc(), &rows, 1_060, Duration::from_secs(1))
            .resolve(bounds(), 0.01);
        assert_eq!(
            resolved.reading.map(|r| r.value),
            Some(1.1410),
            "EURC must compose from its own pair, not another market's"
        );
    }

    /// A pinned market offers no crypto candidate at all.
    #[test]
    fn a_pinned_market_offers_no_crypto_candidate() {
        // A row for its pair exists in the store — the candle collector rosters
        // `AUDD-USDC` — so the emptiness has to come from the roster's pin
        // rather than from an absent row.
        let rows = vec![candle(SOURCE_COINBASE, "AUDD-USDC", 1_000, 0.7220)];
        let resolved = crypto_candidates(&audd(), &rows, 1_060, Duration::from_secs(1))
            .resolve(bounds(), 0.01);
        assert!(
            resolved.reading.is_none(),
            "a pinned market must not offer a basis candidate"
        );
    }

    /// The peg leg reads `USDC-USD` from the tick rows.
    #[test]
    fn the_peg_leg_reads_the_tick_table_series() {
        let rows = vec![
            SpotTickRow {
                source: SOURCE_KRAKEN.to_string(),
                product_id: USDC_USD_PRODUCT.to_string(),
                observed_at: 1_000,
                price: 0.9998,
            },
            SpotTickRow {
                source: SOURCE_KRAKEN.to_string(),
                product_id: "EURC-USDC".to_string(),
                observed_at: 1_000,
                price: 1.14,
            },
        ];
        let resolved = peg_candidates(&rows, 1_060, Duration::from_secs(1)).resolve(bounds(), 0.01);
        assert_eq!(resolved.reading.map(|r| r.value), Some(0.9998));
    }

    /// An empty peg leg is "the guard cannot fire", not a fault.
    ///
    /// Pinned because the peg leg lives in the second table: a `spot_ticks` read
    /// that returns nothing must not dark the whole tick.
    #[test]
    fn an_absent_peg_still_composes() {
        let candles = vec![
            candle("oanda", "EUR-USD", 1_000, 1.1400),
            candle(SOURCE_COINBASE, "EURC-USDC", 1_000, 1.1410),
        ];
        let snapshot = Snapshot {
            candles,
            ticks: Vec::new(),
            read_at: None,
        };
        let market = eurc();
        let legs = build_legs(
            &market,
            &snapshot,
            1_060,
            Duration::from_secs(1),
            Candidates::none(),
        );
        let mut engine = FairValueEngine::new(market.config());
        let fair = engine.compose(legs, Duration::from_secs(1), ClockCtx::in_session());
        assert!(
            fair.fair.is_some(),
            "an absent peg leg must not pause the market"
        );
        assert!(!fair.usdc_breach, "an absent peg cannot breach");
    }

    /// A pinned market composes on its FX anchor with the pinned basis.
    #[test]
    fn a_pinned_market_anchors_on_fx() {
        let snapshot = Snapshot {
            candles: vec![candle("oanda", "AUD-USD", 1_000, 0.7200)],
            ticks: Vec::new(),
            read_at: None,
        };
        let market = audd();
        let legs = build_legs(
            &market,
            &snapshot,
            1_060,
            Duration::from_secs(1),
            Candidates::none(),
        );
        let mut engine = FairValueEngine::new(market.config());
        let fair = engine.compose(legs, Duration::from_secs(1), ClockCtx::in_session());
        assert_eq!(fair.anchor, Anchor::Fx);
        assert_eq!(fair.regime, Regime::FxPinned);
        // Compared within a tolerance, not for equality: `fx × 1.0` runs through
        // the fusion estimator, so the result is 0.7200000000000001 rather than
        // the literal. A tolerance states the claim actually being made — the
        // pinned basis leaves the anchor unchanged — where an exact comparison
        // would be pinning one float's last bit.
        let fair_value = fair.fair.expect("a pinned market still composes a mid");
        assert!(
            (fair_value - 0.72).abs() < 1e-9,
            "fair = fx × 1.0 for a 1:1 redemption peg, got {fair_value}"
        );
    }

    /// The halt reason carries the publish class rather than re-encoding it.
    #[test]
    fn a_halt_names_its_cause() {
        assert_eq!(
            Halt::Publish {
                class: PublishError::Permanent(anyhow!("refused")).class()
            }
            .reason(),
            "publish:permanent"
        );
        assert_eq!(
            Halt::Publish {
                class: PublishError::Transient(anyhow!("dropped")).class()
            }
            .reason(),
            "publish:transient"
        );
        assert_eq!(Halt::StoreSilent.reason(), "store_silent");
    }

    /// The transient window must not exceed the store-silence bound.
    ///
    /// If it did, a store outage that also broke publishing would be reported as
    /// a publish halt rather than as the read failure it is — the operator would
    /// go looking at the wrong end of the pipe.
    #[test]
    fn the_publish_window_does_not_outlast_the_store_bound() {
        assert!(MAX_PUBLISH_RETRY_WINDOW <= crate::fx_store::MAX_STORE_SILENCE);
    }

    /// A configured zero is clamped, because a sub-second tick would collide
    /// with itself on `fair_price`'s `(product_id, ts)` key — publishing nothing
    /// and raising the estimator's own duplicate-publisher alarm against itself.
    #[test]
    fn a_zero_tick_interval_is_clamped() {
        assert_eq!(tick_interval_from_secs(Some(0)), Duration::from_secs(1));
        assert_eq!(tick_interval_from_secs(Some(30)), Duration::from_secs(30));
        assert_eq!(
            tick_interval_from_secs(None),
            Duration::from_secs(DEFAULT_TICK_INTERVAL_SECS),
            "an unset variable takes the default rather than the floor"
        );
    }

    /// The receipt floor grows while reads are failing, which is what ages a
    /// stale snapshot's legs out.
    #[test]
    fn a_cached_snapshot_ages_on_the_receipt_floor() {
        let read_at = Instant::now();
        let snapshot = Snapshot {
            candles: Vec::new(),
            ticks: Vec::new(),
            read_at: Some(read_at),
        };
        let later = read_at + Duration::from_secs(90);
        assert_eq!(snapshot.receipt_age(later), Duration::from_secs(90));
        // Before the first read there is no floor to apply.
        assert_eq!(Snapshot::default().receipt_age(later), Duration::ZERO);
    }
}
