// cspell:word misprice
//! Maker operational telemetry — the read side of running this bot
//! (docs/market-making.md §6).
//!
//! One sample per market per tick, one row per feed leg, and one liveness row
//! per registered feed source, written to the shared `dropset` Postgres
//! through the `feeds` store-sink path and rendered by the provisioned Grafana
//! dashboards in `market-data/grafana/`. Nothing here is read back: the bot
//! makes no decision from these tables, so the whole module is a tap on state
//! the quote loop already computed.
//!
//! **Three properties define the design, and they are all about not harming
//! the thing being observed.**
//!
//! *The quote loop never blocks on the database.* The tick loop is
//! synchronous (`std::thread::sleep`, no runtime — see this crate's
//! `Cargo.toml`), and Postgres is async. So [`Telemetry::emit`] only ever
//! `try_send`s onto a bounded channel and returns; a background task on the
//! same runtime that drives the price feeds drains it and writes. A full
//! channel drops the sample rather than waiting, which is the correct trade:
//! a maker that stalls its quote loop behind a slow write is a worse outcome
//! than a gap in a dashboard.
//!
//! *A database outage never stops telemetry permanently.* The runner's
//! contract is that a sink error propagates and the process crashes to be
//! resumed from its cursor. Applied here that would mean the first blip kills
//! the telemetry runner and the bot then runs blind for the rest of its life,
//! so the store sink is wrapped in [`dropset_feeds::BestEffortSink`], which
//! drops the failed batch, logs, and keeps the runner alive to write the next
//! one.
//!
//! *The schema fence is never called.* `dropset-db-schema`'s
//! `require_schema` is for DB-primary apps; this bot is not one. A database
//! that is unreachable, or behind on its migrations, must degrade to "no
//! telemetry" and never to "refuses to quote" — which is exactly what
//! [`spawn`] returning a disabled handle does.
//!
//! The cost of those three together is that delivery is **at-most-once**: a
//! dropped sample is gone. That is sound only because every record here is a
//! *sample of current state* that the next tick supersedes — which is also
//! why none of this may be reused for the fill/event path, where the records
//! are the product.

use crate::config::LadderLevel;
use crate::context::{Context, ProfileKind, VaultSnapshot};
use crate::model::inventory::Inventory;
use crate::model::killswitch::Action;
use crate::model::ladder::Side;
use anyhow::Result;
use async_trait::async_trait;
use dropset_fair_value::{Candidates, FairValue, FusionReport, Legs};
// `MAX_ERROR_CHARS` bounds the tick-error text a sample carries. Taken from
// the framework rather than restated, so the two error columns cannot drift
// apart — see its own doc there for why the bound is a character count.
use dropset_feeds::{
    connect_lazy, run_until, sanitize_error, BestEffortSink, ChannelSource, HealthOutcome,
    HealthReporter, HealthUpdate, RunConfig, Sink, StoreSink, StoreWriter, MAX_ERROR_CHARS,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::mpsc;

/// The environment variable carrying the shared database's connection string.
/// Absent means telemetry is off, which is the default for a plain localnet
/// run — see [`spawn`].
pub const DATABASE_URL_ENV: &str = "DROPSET_DATABASE_URL";

/// The telemetry channel's depth, in records.
///
/// Sized for the drain being briefly stalled rather than for a backlog: at a
/// 5 s tick over a six-market roster a tick offers ~12 records (a sample and a
/// leg batch per market), so this holds a couple of minutes of ticks. Past
/// that the drain is not slow, it is gone — and buffering a longer history of
/// a *current-state* signal would only mean writing staler rows later.
const CHANNEL_CAP: usize = 512;

/// After the first dropped record, report once every this many. Same shape and
/// reasoning as the framework's health reporter.
const DROP_REPORT_EVERY: u64 = 100;

// The `maker_legs.leg` values — the leg's **role** in the §1 composition,
// never a venue. Each names a candidate set the engine resolves by consensus,
// so there is no answering venue for a leg to be labelled with; how well the
// set agreed is `LegSample::consensus_state`.

/// The FX leg: the currency pair's own rate.
pub const LEG_FX: &str = "fx";
/// The crypto leg: the token priced in USDC.
pub const LEG_CRYPTO_USDC: &str = "crypto_usdc";
/// The peg leg: USDC's own USD value, whose drift the basis absorbs.
pub const LEG_USDC_USD: &str = "usdc_usd";

/// One market's state as of one tick — the `maker_telemetry` row.
///
/// Every field an early-returning tick cannot know is an `Option`, and the
/// migration's column nullability mirrors this exactly. See
/// `db-schema/migrations/0003_maker_telemetry.sql` for which tick paths know
/// what, and why a dashboard must not plot these NULLs as zero.
#[derive(Clone, Debug)]
pub struct Sample {
    pub ts: i64,
    pub market: String,
    pub market_pubkey: String,
    pub base_decimals: i16,
    pub quote_decimals: i16,
    pub fair: Option<f64>,
    pub reference: Option<f64>,
    pub last_set_price: Option<f64>,
    pub on_chain_reference: Option<f64>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub skew_bps: Option<f64>,
    pub anchor: String,
    pub regime: String,
    pub health: String,
    pub degraded: bool,
    pub uncertain: bool,
    pub basis: Option<f64>,
    pub basis_breach: bool,
    pub usdc_breach: bool,
    pub action: String,
    pub halt_reason: Option<String>,
    pub profile_kind: String,
    pub base_value_usd: Option<f64>,
    pub quote_value_usd: Option<f64>,
    pub tvl_usd: Option<f64>,
    pub launch_tvl_usd: Option<f64>,
    pub frozen: Option<bool>,
    pub reference_valid: Option<bool>,
    pub tick_error: Option<String>,
}

/// One feed leg's resolved reading as of one tick — the `maker_legs` row.
///
/// **There is deliberately no "which feed supplied this" field.** A leg is a
/// *candidate set* resolved by consensus, not a ladder with one winning tier,
/// so there is no single answering source to record — and the resolver exposes
/// no contributor list yet. Recording one name would mean picking arbitrarily
/// and presenting the pick as authoritative. What replaces it is the three
/// diagnostics below: how well corroborated the leg was, by how many sources,
/// and who the suspect is when they disagreed.
#[derive(Clone, Debug)]
pub struct LegSample {
    pub ts: i64,
    pub market: String,
    pub leg: String,
    /// The resolved value — the consensus summary of the whole candidate set,
    /// not any one source's print.
    pub value: f64,
    pub age_secs: f64,
    pub confidence: Option<f64>,
    pub fresh: bool,
    /// The consensus state, as the Rust variant's `Debug` name. Six values,
    /// and every reader must enumerate all six: `Absent`, `Corroborated`,
    /// `Agreed`, `SingleTrusted`, `SingleUnverified`, `Dispersed`. The last
    /// two are **not** interchangeable — `SingleUnverified` is the steady
    /// state for a market with no second source, and collapsing it would
    /// erase the "quoting off one unchecked feed" signal.
    pub consensus_state: String,
    /// How many healthy sources contributed to the resolution.
    pub contributor_count: i32,
    /// The source **furthest from** the consensus, when the set was dispersed
    /// — the suspect, the *least* representative member of the set.
    ///
    /// Emphatically **NOT** "the feed that answered": this names the source an
    /// operator should distrust, so that reading is exactly backwards. `None`
    /// whenever the leg was not dispersed, which is the normal case.
    pub dispersion_outlier: Option<String>,
    /// The leg's **fused** estimate — what the composition actually priced off,
    /// as opposed to `value`, which is the fast consensus that guards
    /// dislocations. The two are different numbers with different jobs and both
    /// are recorded, because their gap is the estimator's whole contribution.
    ///
    /// `None` for the USDC peg leg, which is not fused: it feeds a band check
    /// rather than a price, and a guard that must fire on any bad reading has
    /// no business reading a smoothed one.
    pub fused_value: Option<f64>,
    /// Standard deviation of the fused estimate, in the leg's own units — the
    /// quantity a spread-width model wants.
    ///
    /// `None` wherever `fused_value` is, **and independently** when the sigma
    /// itself is non-finite — so a consumer drawing a ±sigma band must tolerate
    /// a NULL sigma beside a present `fused_value` rather than assuming the two
    /// travel together.
    pub fused_sigma: Option<f64>,
    /// What the fusion did this tick, as the Rust variant's `Debug` name, in
    /// the same convention every other enum-ish column uses. Four values:
    /// `Carried`, `Seeded`, `Fused`, and `Reseeded` — the last carrying the size
    /// of the dislocation it adopted.
    pub fusion_step: Option<String>,
    /// How many sources were actually fused, which is **not**
    /// `contributor_count`: that counts the fast consensus, while this counts
    /// the fusion's admitted set. They differ in both directions — a reference
    /// fix is fused but does not corroborate, and a trimmed outlier corroborates
    /// the count but is not fused.
    pub fused_count: Option<i32>,
}

/// The `mechanism` value this writer emits. The consensus attribution the
/// resolver also exposes is a separate follow-up and writes no rows yet — see
/// the migration for why the discriminator exists ahead of its second value.
pub const MECHANISM_FUSION: &str = "fusion";

/// One source's share of one leg's fused estimate — the `maker_leg_contributions`
/// row.
///
/// This is the per-source attribution the schema has been waiting for, and it is
/// specifically the **fusion** one: how much each source's precision moved the
/// estimate the composition priced off. The resolver's own `Contributor` set is
/// a different attribution of the same leg-tick — the exact linear combination
/// behind the *fast consensus* — and lands under its own `mechanism` later.
#[derive(Clone, Debug)]
pub struct ContributionSample {
    pub ts: i64,
    pub market: String,
    pub leg: String,
    /// The `feeds` source name. Joins to `feed_health`, with the same caveat
    /// `maker_legs.dispersion_outlier` carries: a spot source is named per
    /// product (`coinbase:EURC-USDC`) while the resolver offers the bare venue,
    /// so that join is a prefix match on the `:`, not equality.
    pub source: String,
    /// Which attribution this row is — [`MECHANISM_FUSION`] today.
    pub mechanism: String,
    /// What this source read.
    pub value: f64,
    /// The measurement variance it was fused at. `None` for a mechanism with no
    /// such notion, never zero — a zero would read as perfect certainty.
    pub variance: Option<f64>,
    /// Its share of the posterior information, in `[0, 1]`.
    ///
    /// **Zero is meaningful and common**: it is a source that answered and was
    /// *excluded* by the trim for sitting outside the dispersion band. Its
    /// `value` is the interesting number on such a row — it is what the
    /// estimator declined to believe. A reader filtering `weight > 0` is
    /// discarding exactly the disagreements this table exists to surface.
    pub weight: f64,
}

/// What the telemetry channel carries.
///
/// One channel for all three kinds, so a tick's sample, its legs, and any feed
/// liveness that landed alongside are written in **one transaction** by one
/// [`StoreWriter`] — rather than three runners racing three connections to
/// describe the same instant.
/// `Sample` is boxed because it is an order of magnitude wider than the other
/// two variants, and an unboxed enum is sized for its largest: every queued
/// record — including a one-word health update — would otherwise reserve the
/// full sample's footprint, across a channel [`CHANNEL_CAP`] deep.
#[derive(Debug)]
pub enum Record {
    Sample(Box<Sample>),
    Legs(Vec<LegSample>),
    Contributions(Vec<ContributionSample>),
    Health(HealthUpdate),
}

impl From<HealthUpdate> for Record {
    fn from(update: HealthUpdate) -> Self {
        Record::Health(update)
    }
}

/// The handle the synchronous tick loop holds.
///
/// Cheap to clone (one channel sender and one counter), so each market's
/// [`Context`] can carry its own. A `None` sender is the disabled state: every
/// `emit` is then a branch and a return, which is what a run with no database
/// configured pays.
#[derive(Clone)]
pub struct Telemetry {
    tx: Option<mpsc::Sender<Record>>,
    dropped: Arc<AtomicU64>,
}

impl Telemetry {
    /// A handle that discards everything — no database configured, or the
    /// connection failed at startup.
    pub fn disabled() -> Self {
        Self {
            tx: None,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    /// How many records have been dropped for a full channel.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// A [`HealthReporter`] onto this channel, for the price and fill feed
    /// runners.
    ///
    /// This is the generic auto-registration the schema depends on: whatever
    /// source it is handed reports under that source's own name, so a venue
    /// adapter added later gets a `feed_health` row without this crate naming
    /// it. A disabled handle yields `None`, and the caller then drives the
    /// source without metrics.
    pub fn health_reporter(&self) -> Option<HealthReporter<Record>> {
        self.tx.clone().map(HealthReporter::new)
    }

    /// Offer one record, dropping it if the channel is full or the drain is
    /// gone.
    ///
    /// Takes `&self` and never fails, deliberately: a telemetry call that
    /// could return an error would eventually be `?`-propagated into the tick
    /// it is reporting on, which is how an observability path takes down the
    /// thing it observes.
    /// A dropped sample is reported on the first of a run and every Nth after
    /// it, matching the framework's health reporter. Counting silently was the
    /// original shape and it defeats the purpose: a stalled drain then loses
    /// samples with no operator signal anywhere, which reads on the dashboard
    /// as a bot that is merely idle — the exact confusion this telemetry
    /// exists to remove.
    pub fn emit(&self, record: Record) {
        let Some(tx) = &self.tx else {
            return;
        };
        if tx.try_send(record).is_err() {
            let dropped = self.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if dropped == 1 || dropped.is_multiple_of(DROP_REPORT_EVERY) {
                eprintln!(
                    "[telemetry] dropped {dropped} record(s) — the drain is \
                     behind or gone; samples are being lost"
                );
            }
        }
    }
}

/// Build the telemetry pipeline on `rt` and return the handle the tick loop
/// emits through.
///
/// Returns a **disabled** handle rather than an error when
/// [`DATABASE_URL_ENV`] is unset or the pool cannot be built. That is the
/// whole soft-dependency stance in one place: a maker whose telemetry database
/// is down still has a vault to quote, and refusing to start would convert a
/// dashboard outage into a trading outage.
pub fn spawn(rt: &Runtime) -> Telemetry {
    let Ok(url) = std::env::var(DATABASE_URL_ENV) else {
        println!(
            "[telemetry] {DATABASE_URL_ENV} is unset — running without \
             operational telemetry"
        );
        return Telemetry::disabled();
    };

    // A **lazy** pool, which is the difference between telemetry that
    // survives a cold start and telemetry that does not. The maker is not
    // ordered after Postgres (deliberately — see the compose service), so an
    // eager connect here would routinely lose that race and disable telemetry
    // for the whole life of the process over one refused connection at second
    // zero. Deferring means every batch retries, and the best-effort sink
    // turns "not up yet" into a few dropped samples instead.
    //
    // So the only disabling condition left is an unset variable, and the only
    // error is a malformed URL — a misconfiguration worth reporting rather
    // than a transient the bot should ride out.
    let pool = match connect_lazy(&url) {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!(
                "[telemetry] {DATABASE_URL_ENV} is not a usable connection \
                 string ({e:#}) — running without operational telemetry"
            );
            return Telemetry::disabled();
        }
    };

    let (source, tx) = ChannelSource::<Record>::new("maker-telemetry", CHANNEL_CAP);
    let store = StoreSink::new(pool, "maker-telemetry", TelemetryWriter);
    let sinks: Vec<Box<dyn Sink<Record>>> =
        vec![Box::new(BestEffortSink::new("maker telemetry", store))];

    // `std::future::pending` as the shutdown, matching how this crate spawns
    // its price feeds: installing a signal handler on the background runtime
    // would swallow the ctrl-c that stops the synchronous tick loop.
    rt.spawn(async move {
        if let Err(e) = run_until(
            source,
            sinks,
            RunConfig::default(),
            std::future::pending::<()>(),
        )
        .await
        {
            eprintln!("[telemetry] runner exited: {e:#}");
        }
    });

    println!("[telemetry] writing operational telemetry to the shared database");
    Telemetry {
        tx: Some(tx),
        dropped: Arc::new(AtomicU64::new(0)),
    }
}

/// The record → table mapping. The framework owns the transaction and the
/// (unused, for a live stream) cursor; this owns the SQL.
pub struct TelemetryWriter;

#[async_trait]
impl StoreWriter for TelemetryWriter {
    type Record = Record;

    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[Record],
    ) -> Result<u64> {
        let mut written = 0;
        for record in records {
            written += match record {
                Record::Sample(s) => write_sample(tx, s).await?,
                Record::Legs(legs) => {
                    let mut n = 0;
                    for leg in legs {
                        n += write_leg(tx, leg).await?;
                    }
                    n
                }
                Record::Contributions(rows) => {
                    let mut n = 0;
                    for row in rows {
                        n += write_contribution(tx, row).await?;
                    }
                    n
                }
                Record::Health(update) => write_health(tx, update).await?,
            };
        }
        Ok(written)
    }
}

async fn write_sample(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, s: &Sample) -> Result<u64> {
    let res = sqlx::query(include_str!("../queries/maker_telemetry_insert.sql"))
        .bind(s.ts)
        .bind(&s.market)
        .bind(&s.market_pubkey)
        .bind(s.base_decimals)
        .bind(s.quote_decimals)
        .bind(s.fair)
        .bind(s.reference)
        .bind(s.last_set_price)
        .bind(s.on_chain_reference)
        .bind(s.best_bid)
        .bind(s.best_ask)
        .bind(s.skew_bps)
        .bind(&s.anchor)
        .bind(&s.regime)
        .bind(&s.health)
        .bind(s.degraded)
        .bind(s.uncertain)
        .bind(s.basis)
        .bind(s.basis_breach)
        .bind(s.usdc_breach)
        .bind(&s.action)
        .bind(&s.halt_reason)
        .bind(&s.profile_kind)
        .bind(s.base_value_usd)
        .bind(s.quote_value_usd)
        .bind(s.tvl_usd)
        .bind(s.launch_tvl_usd)
        .bind(s.frozen)
        .bind(s.reference_valid)
        .bind(&s.tick_error)
        .execute(&mut **tx)
        .await?;
    Ok(res.rows_affected())
}

async fn write_leg(tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, leg: &LegSample) -> Result<u64> {
    let res = sqlx::query(include_str!("../queries/maker_legs_insert.sql"))
        .bind(leg.ts)
        .bind(&leg.market)
        .bind(&leg.leg)
        .bind(leg.value)
        .bind(leg.age_secs)
        .bind(leg.confidence)
        .bind(leg.fresh)
        .bind(&leg.consensus_state)
        .bind(leg.contributor_count)
        .bind(&leg.dispersion_outlier)
        .bind(leg.fused_value)
        .bind(leg.fused_sigma)
        .bind(&leg.fusion_step)
        .bind(leg.fused_count)
        .execute(&mut **tx)
        .await?;
    Ok(res.rows_affected())
}

async fn write_contribution(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    row: &ContributionSample,
) -> Result<u64> {
    let res = sqlx::query(include_str!(
        "../queries/maker_leg_contributions_insert.sql"
    ))
    .bind(row.ts)
    .bind(&row.market)
    .bind(&row.leg)
    .bind(&row.source)
    .bind(&row.mechanism)
    .bind(row.value)
    .bind(row.variance)
    .bind(row.weight)
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected())
}

/// Upsert one feed's liveness. The two outcomes are different statements, not
/// one with NULLs: a success must not touch `last_error*` and a failure must
/// not touch `last_ok_at` — see the queries for why each omission matters.
async fn write_health(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    update: &HealthUpdate,
) -> Result<u64> {
    let res = match &update.outcome {
        HealthOutcome::Ok { records, caught_up } => {
            sqlx::query(include_str!("../queries/feed_health_ok.sql"))
                .bind(&update.feed)
                .bind(update.at)
                // The column is INTEGER; a batch never approaches i32::MAX,
                // but saturate rather than wrap so a pathological one cannot
                // land a negative count.
                .bind(i32::try_from(*records).unwrap_or(i32::MAX))
                .bind(caught_up)
                .execute(&mut **tx)
                .await?
        }
        HealthOutcome::Error(error) => {
            sqlx::query(include_str!("../queries/feed_health_error.sql"))
                .bind(&update.feed)
                .bind(update.at)
                .bind(error)
                .execute(&mut **tx)
                .await?
        }
    };
    Ok(res.rows_affected())
}

/// Build the per-leg rows for one market's tick, resolving each leg's
/// candidate set under the same bounds the engine is about to apply.
///
/// **Why resolve here rather than read the composed reference.** The engine
/// reports a [`dropset_fair_value::LegReport`] for the FX and basis legs only
/// — the USDC peg leg has none. And the peg is exactly the leg whose quiet
/// disagreement matters most: when it cannot resolve, the basis leg loses
/// Kraken as a candidate, so a peg the operator never sees costs the basis a
/// source. Resolving all three here is the only way that shows up.
///
/// The bounds passed in are the engine's own (`leg_stale`,
/// `leg_dispersion`), so this cannot disagree with the resolution the
/// composition performs a line later.
///
/// A leg that resolved to nothing contributes **no row**: absence is absence,
/// not a zero. But note a leg can be `Absent` *with* candidates that were all
/// stale, and that case does produce no row either — the staleness signal for
/// it lives in `feed_health`, which is per source rather than per leg.
///
/// `fair` supplies each fused leg's estimate. Only the two legs the composition
/// prices off are fused, so the peg leg's fusion fields stay `None` — which is
/// the honest record, not a gap: that leg feeds a band check rather than a
/// price, and a guard whose job is to fire on any bad reading must not be
/// reading a smoothed one.
///
/// Note the consequence of the early return below for a fused leg that resolved
/// to **nothing**: no row is written at all, so a `Carried` estimate on an
/// absent leg is not recorded. The composition may still be pricing off that
/// carried estimate, so `maker_telemetry.fair` can be non-NULL across a tick
/// where this table has no row for the leg that produced it.
pub fn leg_samples(
    ts: i64,
    market: &str,
    legs: &Legs,
    stale_after: std::time::Duration,
    dispersion_frac: f64,
    fair: &FairValue,
) -> Vec<LegSample> {
    let mut out = Vec::with_capacity(3);
    let mut push = |leg: &str, candidates: Candidates, fusion: Option<&FusionReport>| {
        let resolved = candidates.resolve(stale_after, dispersion_frac);
        let Some(r) = resolved.reading else {
            return;
        };
        // A fusion that has never been seeded has no estimate to report, so the
        // whole group stays NULL rather than reporting a variance for a value
        // that does not exist.
        let fused = fusion.filter(|f| f.value.is_some());
        out.push(LegSample {
            ts,
            market: market.to_string(),
            leg: leg.to_string(),
            value: r.value,
            age_secs: r.age.as_secs_f64(),
            confidence: r.confidence,
            fresh: r.fresh(stale_after),
            consensus_state: format!("{:?}", resolved.state),
            contributor_count: i32::try_from(resolved.n).unwrap_or(i32::MAX),
            dispersion_outlier: resolved.outlier.map(str::to_string),
            fused_value: fused.and_then(|f| f.value),
            fused_sigma: fused
                .and_then(FusionReport::sigma)
                .filter(|s| s.is_finite()),
            fusion_step: fused.map(|f| format!("{:?}", f.step)),
            fused_count: fused.map(|f| i32::try_from(f.n).unwrap_or(i32::MAX)),
        });
    };
    push(LEG_FX, legs.fx, Some(&fair.fx_fusion));
    push(LEG_CRYPTO_USDC, legs.crypto_usdc, Some(&fair.crypto_fusion));
    push(LEG_USDC_USD, legs.usdc_usd, None);
    out
}

/// The per-source attribution rows for one market's tick — one per source that
/// answered a fused leg, carrying the weight it was given.
///
/// **Every source that could be measured gets a row, including the ones the
/// trim excluded** (at weight zero). Writing only the sources that moved the
/// estimate would leave a fused value in the table with no record of what it
/// declined to believe, which is precisely the disagreement an operator needs to
/// see: an official reference rate contradicting the tape is signal, not noise.
///
/// A source whose reading has no establishable variance — non-finite or
/// non-positive — is skipped upstream by the filter and so gets no row at all.
/// That is not the same state as a trimmed source, and conflating the two would
/// read a broken feed as a disagreeing one.
pub fn contribution_samples(ts: i64, market: &str, fair: &FairValue) -> Vec<ContributionSample> {
    let mut out = Vec::new();
    for (leg, report) in [
        (LEG_FX, &fair.fx_fusion),
        (LEG_CRYPTO_USDC, &fair.crypto_fusion),
    ] {
        for c in report.contributions() {
            out.push(ContributionSample {
                ts,
                market: market.to_string(),
                leg: leg.to_string(),
                source: c.source.to_string(),
                mechanism: MECHANISM_FUSION.to_string(),
                value: c.value,
                variance: Some(c.variance),
                weight: c.weight,
            });
        }
    }
    out
}

/// The state a tick reached, which decides its `action` label and how much of
/// the sample is knowable. Built up as the tick proceeds so that one emit at
/// the end covers every path — including the ones that return early, which
/// are exactly the states (a halt, a frozen vault, a failed read) an operator
/// most needs on the timeline.
///
/// **A tick failure is deliberately NOT a variant here.** It is a separate
/// field on the builder, because a tick can both decide *and* fail: the
/// kill-switch policy says `Halt` and then the instruction that takes the book
/// dark errors. Modelling the error as an outcome made the two mutually
/// exclusive, and the error — arriving last — won. That silently rewrote the
/// single most alarming state the bot can be in ("policy halted and the bot
/// could not comply") into a generic `TickError` with a NULL `halt_reason`,
/// which is exactly the row the provisioned kill-switch alert keys
/// `action = 'Halt'` on. It would not have fired.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// Nothing decided yet.
    ///
    /// Reached whenever a tick returns before any of the states below, which
    /// the vault-read failure does on every tick it happens — so this is not
    /// a merely-theoretical variant. Paired with a tick error it renders as
    /// `TickError`; on its own it renders as `Unknown`, which no path
    /// currently produces and which therefore stays available as the loud
    /// signal for a future path added without an outcome, rather than
    /// impersonating a real trading state.
    Undecided,
    /// The vault is frozen on-chain; the bot idles.
    Frozen,
    /// No usable reference — the composition paused.
    Pause,
    /// The kill-switch policy ran and decided.
    Decided(Action),
}

impl Outcome {
    /// The `action` label. Bare variant names, never the `Debug` payload
    /// form: the provisioned alert matches `action = 'Halt'` and the state
    /// timeline maps these exact strings, so `Halt(BasisBreach)` would break
    /// both silently. The payload is carried by `halt_reason` instead.
    ///
    /// Takes `failed` because a tick that reached no decision *and* failed is
    /// the ordinary vault-read timeout, not a mystery: `read_vault` is the
    /// first thing a tick does, so it returns before any outcome is set. That
    /// pairing is what `TickError` names, and every reader — the state
    /// timeline's value mappings, the kill-switch rule's carve-out comment,
    /// and this table's own column comment — was written against it. Emitting
    /// the bare `Unknown` there instead left `TickError` matching nothing and
    /// `Unknown` mapped by nothing, so a market failing every tick rendered
    /// as an unbroken band on the timeline's base style, with no rule firing.
    ///
    /// A *decided* tick that then failed keeps its decision — that is the
    /// invariant this enum exists to defend, and the reason `failed` cannot
    /// simply override.
    fn action(&self, failed: bool) -> &'static str {
        match self {
            Outcome::Undecided if failed => "TickError",
            Outcome::Undecided => "Unknown",
            Outcome::Frozen => "Frozen",
            Outcome::Pause => "Pause",
            Outcome::Decided(Action::Quote) => "Quote",
            Outcome::Decided(Action::Reshape(_)) => "Reshape",
            Outcome::Decided(Action::FreezeSide(_)) => "FreezeSide",
            Outcome::Decided(Action::Halt(_)) => "Halt",
        }
    }

    fn halt_reason(&self) -> Option<String> {
        match self {
            Outcome::Decided(Action::Halt(reason)) => Some(format!("{reason:?}")),
            _ => None,
        }
    }
}

/// Everything one tick learned about one market, assembled incrementally.
///
/// A builder rather than a wide constructor because the tick genuinely learns
/// these in stages and returns from the middle: the vault read, then the mid,
/// then the valued inventory, then the policy decision. Anything not reached
/// stays `None` and lands as SQL NULL.
pub struct SampleBuilder {
    ts: i64,
    market: MarketId,
    fair: FairValue,
    profile_kind: ProfileKind,
    last_set_price: Option<f64>,
    outcome: Outcome,
    /// What the tick failed with, independent of what it decided — see
    /// [`Outcome`] for why these are separate.
    error: Option<String>,
    frozen: Option<bool>,
    reference_valid: Option<bool>,
    on_chain_reference: Option<f64>,
    reference: Option<f64>,
    skew_bps: Option<f64>,
    inventory: Option<Inventory>,
    launch_tvl_usd: Option<f64>,
    tightest_offset_ppm: Option<u32>,
}

impl SampleBuilder {
    /// Start a sample for `market` from what is known before the tick runs:
    /// the composed reference, and the bot's own carried state.
    pub fn new(ts: i64, market: MarketId, fair: FairValue, ctx_profile: ProfileKind) -> Self {
        Self {
            ts,
            market,
            fair,
            profile_kind: ctx_profile,
            last_set_price: None,
            outcome: Outcome::Undecided,
            error: None,
            frozen: None,
            reference_valid: None,
            on_chain_reference: None,
            reference: None,
            skew_bps: None,
            inventory: None,
            launch_tvl_usd: None,
            tightest_offset_ppm: None,
        }
    }

    /// The bot's belief about the reference it last stamped.
    pub fn last_set(&mut self, price: Option<f64>) -> &mut Self {
        self.last_set_price = price;
        self
    }

    /// What the per-tick vault read saw.
    pub fn vault(&mut self, vault: &VaultSnapshot) -> &mut Self {
        self.frozen = Some(vault.frozen);
        self.reference_valid = Some(vault.reference_valid);
        self.on_chain_reference = Some(vault.reference_price);
        self
    }

    /// The valued inventory and the drawdown baseline it is measured against.
    pub fn inventory(&mut self, inv: Inventory, launch_tvl_usd: f64) -> &mut Self {
        self.inventory = Some(inv);
        self.launch_tvl_usd = Some(launch_tvl_usd);
        self
    }

    /// The skewed reference this tick computed, and the skew that produced it.
    pub fn reference(&mut self, reference: f64, skew_bps: f64) -> &mut Self {
        self.reference = Some(reference);
        self.skew_bps = Some(skew_bps);
        self
    }

    /// The ladder in force, whose tightest level sets the implied touch.
    pub fn ladder(&mut self, ladder: &[LadderLevel]) -> &mut Self {
        // The ladder is validated monotonic in `offset_ppm` (see
        // `config::tests::ladder_is_monotonic`), so `min` and "level 0" agree
        // — `min` is used anyway so a future non-monotonic ladder cannot
        // quietly report the wrong touch.
        self.tightest_offset_ppm = ladder.iter().map(|l| l.offset_ppm).min();
        self
    }

    /// What this tick's policy decided.
    pub fn outcome(&mut self, outcome: Outcome) -> &mut Self {
        self.outcome = outcome;
        self
    }

    /// What the tick failed with. Recorded **alongside** the decision, never
    /// instead of it: a tick that halted and then failed to take the book dark
    /// must still report `action = 'Halt'`, because that is what the
    /// kill-switch alert keys on.
    pub fn error(&mut self, error: &anyhow::Error) -> &mut Self {
        self.error = Some(format!("{error:#}"));
        self
    }

    /// The implied best bid / ask of the **resting** book, **as observed at
    /// the start of this tick**.
    ///
    /// Every input is from the per-tick vault read — the on-chain reference,
    /// `reference_valid`, `frozen` — plus the profile shape the bot believed
    /// was armed *before* the tick ran. So the touch is one consistent
    /// snapshot, and an action this tick takes appears in the *next* sample
    /// (5 s later) rather than in this one.
    ///
    /// That lag is deliberate, and the alternative is worse. Mixing this
    /// tick's *decision* into the touch reports a dark book from the instant
    /// the policy decides — but the policy's instruction can fail, and then
    /// the book is still live and matchable while the dashboard says it is
    /// dark. That is the unsafe direction: it under-states risk. Pre-tick
    /// state cannot lie about what was observed, and a halt is separately
    /// visible on the same row the instant it is decided, via `action`.
    ///
    /// A side is `None` when it was dark, in any of four ways: the vault is
    /// frozen, the on-chain reference is not valid (never stamped, or killed
    /// for staleness), the bot had already armed a halt, or it had already
    /// frozen that one side.
    ///
    /// The frozen case rests on a program guarantee worth citing rather than
    /// re-deriving: `freeze_vault` leaves the vault on the active list, so a
    /// frozen vault's levels are still *materialized*, but `swap`'s matching
    /// walk skips frozen vaults entirely — the match-time skip is where the
    /// freeze is enforced. So frozen really does mean unmatchable, not merely
    /// deposit-blocked, and the gate is not over-reporting darkness.
    ///
    /// **`None` here is two-valued and a reader must not conflate them.** It
    /// means "provably dark" only when the tick got far enough to observe the
    /// vault; on the failed-vault-read path it means "unknown", because
    /// `on_chain_reference` was never populated and the first gate returns.
    /// A full ladder can be resting and matchable on such a tick. The
    /// discriminator on the row is `reference_valid IS NOT NULL` — so a
    /// consumer asking "was this book live" must filter on that first, and
    /// the same tick's `tick_error` says why it could not be answered.
    fn touch(&self) -> (Option<f64>, Option<f64>) {
        let (Some(reference), Some(true), Some(offset_ppm)) = (
            self.on_chain_reference,
            self.reference_valid,
            self.tightest_offset_ppm,
        ) else {
            return (None, None);
        };
        if self.frozen == Some(true) {
            return (None, None);
        }
        let offset = f64::from(offset_ppm) / 1_000_000.0;
        let (mut bid, mut ask) = (
            Some(reference * (1.0 - offset)),
            Some(reference * (1.0 + offset)),
        );
        // An armed halt keeps the book dark until a fresh reference re-arms
        // it, whatever a later tick's policy says.
        if self.profile_kind == ProfileKind::Halted {
            return (None, None);
        }
        if let ProfileKind::FrozenSide(side) = self.profile_kind {
            // `FrozenSide(side)` zeroes the side that *accumulates* the heavy
            // leg; only the rebuild side keeps quoting.
            match side {
                Side::Bid => bid = None,
                Side::Ask => ask = None,
            }
        }
        (bid, ask)
    }

    /// Finish the sample.
    pub fn build(self) -> Sample {
        let (best_bid, best_ask) = self.touch();
        Sample {
            ts: self.ts,
            market: self.market.symbol.clone(),
            market_pubkey: self.market.pubkey.clone(),
            base_decimals: self.market.base_decimals,
            quote_decimals: self.market.quote_decimals,
            fair: self.fair.fair,
            reference: self.reference,
            last_set_price: self.last_set_price,
            on_chain_reference: self.on_chain_reference,
            best_bid,
            best_ask,
            skew_bps: self.skew_bps,
            anchor: format!("{:?}", self.fair.anchor),
            regime: format!("{:?}", self.fair.regime),
            health: format!("{:?}", self.fair.health),
            degraded: self.fair.degraded(),
            uncertain: self.fair.uncertain,
            basis: self.fair.basis,
            basis_breach: self.fair.basis_breach,
            usdc_breach: self.fair.usdc_breach,
            action: self.outcome.action(self.error.is_some()).to_string(),
            halt_reason: self.outcome.halt_reason(),
            profile_kind: format!("{:?}", self.profile_kind),
            base_value_usd: self.inventory.map(|i| i.base_value_usd),
            quote_value_usd: self.inventory.map(|i| i.quote_value_usd),
            tvl_usd: self.inventory.map(|i| i.total_usd()),
            launch_tvl_usd: self.launch_tvl_usd,
            frozen: self.frozen,
            reference_valid: self.reference_valid,
            tick_error: self
                .error
                .as_deref()
                .map(|e| sanitize_error(e, MAX_ERROR_CHARS)),
        }
    }
}

/// A market's telemetry identity: how a row is labelled, joined, and scaled.
///
/// Carried as one value rather than four arguments so a caller cannot pair one
/// market's symbol with another's decimals — which would not fail to compile
/// and would silently misprice every fill overlaid on that market.
#[derive(Clone, Debug)]
pub struct MarketId {
    /// The roster symbol — what a dashboard legend reads.
    pub symbol: String,
    /// The market account, base58 — the join key onto the indexer's tables.
    pub pubkey: String,
    pub base_decimals: i16,
    pub quote_decimals: i16,
}

impl MarketId {
    /// Read the identity off a live market's context.
    pub fn of(ctx: &Context) -> Self {
        Self {
            symbol: ctx.cfg.symbol.to_string(),
            pubkey: ctx.market.market.to_string(),
            base_decimals: i16::from(ctx.market.base_decimals),
            quote_decimals: i16::from(ctx.market.quote_decimals),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_LADDER;
    use crate::model::killswitch::HaltReason;
    use dropset_fair_value::{Anchor, FusionReport, Health, LegReport, Reading, Regime};
    use std::time::Duration;

    /// The tightest default ladder tier, as a fraction — the touch offset every
    /// expectation below is built from, derived rather than hardcoded so a
    /// ladder retune cannot silently invalidate the assertions.
    fn touch_offset() -> f64 {
        f64::from(
            DEFAULT_LADDER
                .iter()
                .map(|l| l.offset_ppm)
                .min()
                .expect("the default ladder is non-empty"),
        ) / 1_000_000.0
    }

    fn fair(mid: Option<f64>) -> FairValue {
        FairValue {
            fair: mid,
            anchor: Anchor::Fx,
            regime: Regime::Normal,
            basis: Some(1.001),
            basis_age: Some(Duration::ZERO),
            basis_outlier: false,
            fx_leg: LegReport::default(),
            crypto_leg: LegReport::default(),
            fx_fusion: FusionReport::none(),
            crypto_fusion: FusionReport::none(),
            health: Health::Ok,
            uncertain: false,
            basis_breach: false,
            usdc_breach: false,
        }
    }

    /// A 6-vs-6 market, matching today's roster. The decimals matter to the
    /// dashboards' fill-price scaling, not to anything asserted here.
    fn eurc() -> MarketId {
        MarketId {
            symbol: "EURC".to_string(),
            pubkey: "11111111111111111111111111111111".to_string(),
            base_decimals: 6,
            quote_decimals: 6,
        }
    }

    fn vault(reference_valid: bool, frozen: bool) -> VaultSnapshot {
        VaultSnapshot {
            sector_idx: 0,
            base_atoms: 1_000_000,
            quote_atoms: 1_000_000,
            reference_price: 1.14,
            reference_valid,
            frozen,
        }
    }

    /// A builder in the state the happy path reaches: vault read, inventory
    /// valued, reference computed, policy decided.
    fn quoting(action: Action, profile: ProfileKind) -> SampleBuilder {
        let mut b = SampleBuilder::new(1_786_579_250, eurc(), fair(Some(1.14)), profile);
        b.last_set(Some(1.14))
            .ladder(&DEFAULT_LADDER)
            .vault(&vault(true, false))
            .inventory(
                Inventory {
                    base_value_usd: 600.0,
                    quote_value_usd: 400.0,
                },
                1_000.0,
            )
            .reference(1.14, 2.5)
            .outcome(Outcome::Decided(action));
        b
    }

    #[test]
    fn a_quoting_tick_reports_both_sides_around_the_on_chain_reference() {
        let s = quoting(Action::Quote, ProfileKind::Standard).build();
        let off = touch_offset();
        // The touch is derived from the reference resting *on-chain*, not from
        // the candidate this tick computed — that is what a taker can hit.
        assert_eq!(s.best_bid, Some(1.14 * (1.0 - off)));
        assert_eq!(s.best_ask, Some(1.14 * (1.0 + off)));
        assert_eq!(s.action, "Quote");
        assert_eq!(s.halt_reason, None);
        assert_eq!(s.tvl_usd, Some(1_000.0));
        assert_eq!(s.skew_bps, Some(2.5));
    }

    #[test]
    fn a_halt_names_its_reason_but_does_not_itself_dark_the_touch() {
        let s = quoting(Action::Halt(HaltReason::BasisBreach), ProfileKind::Standard).build();
        assert_eq!(s.action, "Halt");
        assert_eq!(s.halt_reason.as_deref(), Some("BasisBreach"));
        // The touch is the book as OBSERVED at the start of the tick, and at
        // that moment a Standard profile was still armed — so both sides are
        // reported live even though the policy has just decided to halt.
        //
        // This is the safe direction, and the inverse is the bug it replaced:
        // darking on the decision reports an empty book from the instant the
        // policy decides, but the instruction that darks it can FAIL, and then
        // the real book is live and matchable while the dashboard says it is
        // not. Under-stating live risk is the one error worth ruling out
        // structurally. The halt is visible immediately via `action`; the
        // touch catches up on the next tick.
        assert!(s.best_bid.is_some() && s.best_ask.is_some());
    }

    /// A freeze-side that has actually been armed darks the accumulating side.
    /// Keyed on the armed profile, not on the decision — same reasoning as the
    /// halt case above.
    #[test]
    fn an_armed_freeze_side_darks_only_the_accumulating_side() {
        let bid_frozen = quoting(
            Action::FreezeSide(Side::Bid),
            ProfileKind::FrozenSide(Side::Bid),
        )
        .build();
        assert_eq!(bid_frozen.best_bid, None);
        assert!(
            bid_frozen.best_ask.is_some(),
            "the rebuild side keeps quoting"
        );

        let ask_frozen = quoting(
            Action::FreezeSide(Side::Ask),
            ProfileKind::FrozenSide(Side::Ask),
        )
        .build();
        assert!(ask_frozen.best_bid.is_some());
        assert_eq!(ask_frozen.best_ask, None);
    }

    /// The decision alone must never move the touch — pinned separately from
    /// the two cases above so a refactor that reintroduces outcome-driven
    /// darkening fails here with an unambiguous name.
    #[test]
    fn the_touch_ignores_the_decision_and_follows_the_armed_profile() {
        // Policy says freeze the bid, but nothing is armed yet: both live.
        let deciding = quoting(Action::FreezeSide(Side::Bid), ProfileKind::Standard).build();
        assert!(deciding.best_bid.is_some() && deciding.best_ask.is_some());

        // Policy says quote, but a halt is already armed: both dark.
        let armed = quoting(Action::Quote, ProfileKind::Halted).build();
        assert_eq!((armed.best_bid, armed.best_ask), (None, None));
    }

    /// A reshape shrinks the accumulating side's *size*, not its offset, so the
    /// touch is unchanged — the distinction the schema's `profile_kind` column
    /// carries and the touch columns must not conflate with a freeze.
    #[test]
    fn a_reshape_leaves_both_sides_quoting() {
        let s = quoting(Action::Reshape(Side::Bid), ProfileKind::Reshaped(Side::Bid)).build();
        assert_eq!(s.action, "Reshape");
        assert!(s.best_bid.is_some() && s.best_ask.is_some());
    }

    /// The armed profile outranks this tick's policy: a book the bot already
    /// took dark stays dark until a fresh reference re-arms it, even on a tick
    /// whose policy says `Quote`.
    #[test]
    fn an_already_halted_profile_stays_dark_under_a_quote_decision() {
        let s = quoting(Action::Quote, ProfileKind::Halted).build();
        assert_eq!(s.action, "Quote", "the policy decision is reported as-is");
        assert_eq!((s.best_bid, s.best_ask), (None, None));

        let one_side = quoting(Action::Quote, ProfileKind::FrozenSide(Side::Ask)).build();
        assert!(one_side.best_bid.is_some());
        assert_eq!(one_side.best_ask, None);
    }

    #[test]
    fn an_invalid_on_chain_reference_reports_no_touch() {
        let mut b = SampleBuilder::new(1, eurc(), fair(Some(1.14)), ProfileKind::Standard);
        b.ladder(&DEFAULT_LADDER)
            .vault(&vault(false, false))
            .outcome(Outcome::Decided(Action::Quote));
        let s = b.build();
        // Never stamped, or killed for staleness: there is no resting book.
        assert_eq!((s.best_bid, s.best_ask), (None, None));
        assert_eq!(s.reference_valid, Some(false));
    }

    #[test]
    fn a_frozen_vault_reports_the_state_and_no_touch() {
        let mut b = SampleBuilder::new(1, eurc(), fair(Some(1.14)), ProfileKind::Standard);
        b.ladder(&DEFAULT_LADDER)
            .vault(&vault(true, true))
            .outcome(Outcome::Frozen);
        let s = b.build();
        assert_eq!(s.action, "Frozen");
        assert_eq!(s.frozen, Some(true));
        assert_eq!((s.best_bid, s.best_ask), (None, None));
    }

    /// The paused path returns before valuing the vault, so the columns it
    /// could not know must be NULL rather than zero — a zero skew and an
    /// unknown skew are different facts.
    #[test]
    fn a_paused_tick_leaves_what_it_never_computed_null() {
        let mut b = SampleBuilder::new(1, eurc(), fair(None), ProfileKind::Standard);
        b.ladder(&DEFAULT_LADDER)
            .vault(&vault(true, false))
            .outcome(Outcome::Pause);
        let s = b.build();
        assert_eq!(s.action, "Pause");
        assert_eq!(s.fair, None);
        assert_eq!(s.skew_bps, None);
        assert_eq!(s.tvl_usd, None);
        assert_eq!(s.base_value_usd, None);
        assert_eq!(s.launch_tvl_usd, None);
    }

    /// The vault read failing is the one path that knows almost nothing — and
    /// it must still produce a row, because a missing row is indistinguishable
    /// from a dead bot.
    #[test]
    fn a_failed_tick_still_reports_a_sample_carrying_the_error() {
        let mut b = SampleBuilder::new(1, eurc(), fair(Some(1.14)), ProfileKind::Standard);
        b.error(&anyhow::anyhow!("reading the vault: timed out"));
        let s = b.build();
        // Nothing decided *and* a failure, which is what the timeline, the
        // alert carve-out and the column comment all call `TickError`. The
        // bare `Unknown` here left that value matching nothing.
        assert_eq!(s.action, "TickError");
        assert_eq!(
            s.tick_error.as_deref(),
            Some("reading the vault: timed out")
        );
        assert_eq!(s.frozen, None, "the vault was never read");
        assert_eq!((s.best_bid, s.best_ask), (None, None));
        // The composed reference is known before the tick runs, so it is still
        // reported — which is what makes a failing tick diagnosable at all.
        assert_eq!(s.fair, Some(1.14));
        assert_eq!(s.regime, "Normal");
    }

    /// The column list an `INSERT` names, in order, and the highest `$N` it
    /// binds. Enough to catch the two ways these queries drift from the
    /// `.bind()` chains beside them.
    fn insert_shape(sql: &str) -> (Vec<String>, usize) {
        // Anchored on the statement, not on the first paren in the file:
        // these queries carry a leading comment that mentions the conflict
        // key as `(market, ts)`, which is otherwise what gets parsed.
        let stmt = sql.find("INSERT INTO").expect("an INSERT statement");
        let open = sql[stmt..].find('(').expect("an INSERT names its columns") + stmt;
        let close = sql[open..].find(')').expect("unterminated column list") + open;
        let columns = sql[open + 1..close]
            .split(',')
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .collect();
        // `$10` must not read as `$1`, so take the digits, not one char.
        let highest = sql
            .match_indices('$')
            .filter_map(|(i, _)| {
                let digits: String = sql[i + 1..]
                    .chars()
                    .take_while(char::is_ascii_digit)
                    .collect();
                digits.parse::<usize>().ok()
            })
            .max()
            .unwrap_or(0);
        (columns, highest)
    }

    /// The four telemetry writes bind positionally into SQL held in separate
    /// files, so nothing in the compiler relates a column to the value that
    /// lands in it — there is no compile-time database here (deliberately;
    /// see the module doc), and no test touches a live one.
    ///
    /// The failure that motivates this is not a count mismatch, which at
    /// least errors at runtime. It is two *same-typed* adjacent columns
    /// transposed — `reference`/`last_set_price`, `best_bid`/`best_ask`,
    /// `degraded`/`uncertain`, `base_value_usd`/`quote_value_usd`. That
    /// inserts cleanly, forever, and every panel keeps rendering plausible
    /// numbers off the wrong column.
    ///
    /// Be honest about the guarantee: this pins each query's column list and
    /// arity against an expectation stated here, so SQL edited without its
    /// binder (or vice versa) fails. It cannot see a transposition made
    /// consistently in *both* the SQL and this array — closing that would
    /// mean driving the binds from one ordered source, which is the real fix
    /// if these grow again.
    #[test]
    fn every_insert_matches_the_bind_order_beside_it() {
        let cases: [(&str, &str, &[&str], usize); 5] = [
            (
                "maker_telemetry_insert",
                include_str!("../queries/maker_telemetry_insert.sql"),
                &[
                    "ts",
                    "market",
                    "market_pubkey",
                    "base_decimals",
                    "quote_decimals",
                    "fair",
                    "reference",
                    "last_set_price",
                    "on_chain_reference",
                    "best_bid",
                    "best_ask",
                    "skew_bps",
                    "anchor",
                    "regime",
                    "health",
                    "degraded",
                    "uncertain",
                    "basis",
                    "basis_breach",
                    "usdc_breach",
                    "action",
                    "halt_reason",
                    "profile_kind",
                    "base_value_usd",
                    "quote_value_usd",
                    "tvl_usd",
                    "launch_tvl_usd",
                    "frozen",
                    "reference_valid",
                    "tick_error",
                ],
                30,
            ),
            (
                "maker_legs_insert",
                include_str!("../queries/maker_legs_insert.sql"),
                &[
                    "ts",
                    "market",
                    "leg",
                    "value",
                    "age_secs",
                    "confidence",
                    "fresh",
                    "consensus_state",
                    "contributor_count",
                    "dispersion_outlier",
                    "fused_value",
                    "fused_sigma",
                    "fusion_step",
                    "fused_count",
                ],
                14,
            ),
            (
                "maker_leg_contributions_insert",
                include_str!("../queries/maker_leg_contributions_insert.sql"),
                &[
                    "ts",
                    "market",
                    "leg",
                    "source",
                    "mechanism",
                    "value",
                    "variance",
                    "weight",
                ],
                8,
            ),
            (
                "feed_health_ok",
                include_str!("../queries/feed_health_ok.sql"),
                &[
                    "feed",
                    "status",
                    "last_ok_at",
                    "last_records",
                    "caught_up",
                    "ok_count",
                    "updated_at",
                ],
                // Four distinct binds; `$2` is reused for `updated_at`, which
                // is what keeps it equal to the outcome's stamp by
                // construction rather than by a second `now()`.
                4,
            ),
            (
                "feed_health_error",
                include_str!("../queries/feed_health_error.sql"),
                &[
                    "feed",
                    "status",
                    "last_error_at",
                    "last_error",
                    "error_count",
                    "updated_at",
                ],
                3,
            ),
        ];

        for (name, sql, expected, binds) in cases {
            let (columns, highest) = insert_shape(sql);
            assert_eq!(columns, expected, "{name}: column list drifted");
            assert_eq!(highest, binds, "{name}: placeholder count drifted");
        }
    }

    /// `Unknown` is reserved for the shape that should not exist — no
    /// decision and no failure — so it stays a defect signal rather than the
    /// label of the ordinary vault-read timeout. Pinned alongside the test
    /// above because the two values are one boolean apart, and swapping them
    /// is invisible: both are strings a panel renders without complaint.
    #[test]
    fn a_tick_that_neither_decided_nor_failed_is_the_loud_placeholder() {
        let b = SampleBuilder::new(1, eurc(), fair(Some(1.14)), ProfileKind::Standard);
        let s = b.build();
        assert_eq!(s.action, "Unknown");
        assert_eq!(s.tick_error, None);
    }

    /// The column and the label do not partition, and a query that assumed
    /// they did would miss the most alarming row the bot can write.
    #[test]
    fn a_decided_tick_that_failed_keeps_its_decision_and_its_error() {
        let mut b = SampleBuilder::new(1, eurc(), fair(Some(1.14)), ProfileKind::Standard);
        b.outcome(Outcome::Decided(Action::Halt(HaltReason::BasisBreach)))
            .error(&anyhow::anyhow!(
                "sending the kill stamp: blockhash expired"
            ));
        let s = b.build();
        assert_eq!(s.action, "Halt", "the decision wins the column");
        assert_eq!(s.halt_reason.as_deref(), Some("BasisBreach"));
        assert!(s.tick_error.is_some(), "and the failure is still recorded");
    }

    /// The regression this pairs with a real bug: a tick whose policy halted
    /// and whose kill stamp then failed must report BOTH. Recording the error
    /// as the outcome erased `action = 'Halt'` and the halt reason, which is
    /// exactly what the provisioned kill-switch alert matches on — so the
    /// alert would not have fired on the worst row the bot can produce.
    #[test]
    fn an_error_after_a_decision_keeps_the_decision() {
        let mut b = quoting(Action::Halt(HaltReason::BasisBreach), ProfileKind::Standard);
        b.error(&anyhow::anyhow!("kill stamp failed: blockhash expired"));
        let s = b.build();
        assert_eq!(s.action, "Halt", "the alert keys on this");
        assert_eq!(s.halt_reason.as_deref(), Some("BasisBreach"));
        assert!(s.tick_error.is_some(), "and the failure is still recorded");
    }

    #[test]
    fn a_long_tick_error_is_truncated() {
        let mut b = SampleBuilder::new(1, eurc(), fair(None), ProfileKind::Unknown);
        b.error(&anyhow::anyhow!("{}", "x".repeat(MAX_ERROR_CHARS + 50)));
        let error = b.build().tick_error.expect("an error was set");
        assert_eq!(error.chars().count(), MAX_ERROR_CHARS + 1);
        assert!(error.ends_with('…'));
    }

    /// A credential carried in a URL query string must not reach a column the
    /// read-only dashboard role can read.
    #[test]
    fn a_tick_error_redacts_a_keyed_url() {
        let mut b = SampleBuilder::new(1, eurc(), fair(None), ProfileKind::Unknown);
        b.error(&anyhow::anyhow!(
            "vault read failed: https://rpc.example/?api-key=SECRET timed out"
        ));
        let error = b.build().tick_error.expect("an error was set");
        assert!(!error.contains("SECRET"), "got: {error}");
        assert!(error.contains("https://rpc.example/?<redacted>"));
        // The diagnosable part survives.
        assert!(error.contains("timed out"));
    }

    /// A leg offered by one named source, fresh as of this tick.
    fn one(source: &'static str, value: f64) -> Candidates {
        Candidates::none().push(source, Some(Reading::new(value, Duration::from_secs(1))))
    }

    const BAND: f64 = 0.01;
    const STALE: std::time::Duration = Duration::from_secs(15);

    #[test]
    fn leg_samples_record_the_consensus_and_skip_legs_that_resolved_to_nothing() {
        let legs = Legs {
            fx: Candidates::none().push(
                "pyth-hermes",
                Some(Reading::with_confidence(
                    1.14,
                    Duration::from_secs(2),
                    0.0004,
                )),
            ),
            crypto_usdc: one("coinbase", 1.1401),
            // Nothing offered at all, so nothing resolves.
            usdc_usd: Candidates::none(),
            static_usd: 1.14,
        };

        let rows = leg_samples(42, "EURC", &legs, STALE, BAND, &fair(None));
        assert_eq!(rows.len(), 2, "the empty peg leg contributes no row");

        let fx = &rows[0];
        assert_eq!(fx.leg, LEG_FX);
        assert_eq!(fx.confidence, Some(0.0004));
        assert_eq!(fx.age_secs, 2.0);
        assert!(fx.fresh);
        assert_eq!(fx.contributor_count, 1);
        assert_eq!(fx.dispersion_outlier, None, "a lone source cannot disperse");

        let crypto = &rows[1];
        assert_eq!(crypto.leg, LEG_CRYPTO_USDC);
        // A REST quote publishes no half-width; that is "no notion", never
        // "certain".
        assert_eq!(crypto.confidence, None);
    }

    /// The two single-source states must be distinguishable, because
    /// `SingleUnverified` is the *steady state* for a market with no second
    /// source and is the only signal that a market is quoted off one unchecked
    /// feed. Collapsing them would erase exactly that.
    #[test]
    fn the_two_single_source_states_are_recorded_distinctly() {
        let trusted = Legs {
            fx: Candidates::none()
                .push_trusted("pyth-hermes", Some(Reading::new(1.14, Duration::ZERO))),
            crypto_usdc: Candidates::none(),
            usdc_usd: Candidates::none(),
            static_usd: 1.14,
        };
        let unverified = Legs {
            fx: one("frankfurter", 1.14),
            crypto_usdc: Candidates::none(),
            usdc_usd: Candidates::none(),
            static_usd: 1.14,
        };

        let a = leg_samples(1, "EURC", &trusted, STALE, BAND, &fair(None));
        let b = leg_samples(1, "EURC", &unverified, STALE, BAND, &fair(None));
        assert_eq!(a[0].consensus_state, "SingleTrusted");
        assert_eq!(b[0].consensus_state, "SingleUnverified");
        assert_ne!(a[0].consensus_state, b[0].consensus_state);
    }

    /// The suspect is the source furthest from the consensus — the one to
    /// distrust, never "the feed that answered".
    #[test]
    fn a_dispersed_leg_names_its_outlier_and_counts_its_contributors() {
        let legs = Legs {
            fx: Candidates::none(),
            // Three sources, one of them far out: the median holds and the
            // stray is named.
            crypto_usdc: one("coinbase", 1.140)
                .push("kraken", Some(Reading::new(1.141, Duration::ZERO)))
                .push("coingecko", Some(Reading::new(0.570, Duration::ZERO))),
            usdc_usd: Candidates::none(),
            static_usd: 1.14,
        };

        let rows = leg_samples(1, "EURC", &legs, STALE, BAND, &fair(None));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].consensus_state, "Dispersed");
        assert_eq!(rows[0].contributor_count, 3);
        assert_eq!(rows[0].dispersion_outlier.as_deref(), Some("coingecko"));
        // The median still carries the leg — dispersed is flagged, not
        // discarded, once there are three or more sources.
        assert!(rows[0].value > 1.13);
    }

    /// A composed tick whose basis leg carries a bad print: the fused estimate
    /// is recorded beside the fast consensus, and the trimmed source gets its
    /// own attribution row at weight zero.
    #[test]
    fn a_fused_leg_records_its_estimate_and_every_contributor() {
        use dropset_fair_value::{FairValueConfig, FairValueEngine};

        let legs = Legs {
            fx: one("pyth-hermes", 1.0),
            crypto_usdc: one("coinbase", 1.020)
                .push("kraken", Some(Reading::new(1.021, Duration::ZERO)))
                .push("coingecko", Some(Reading::new(0.530, Duration::ZERO))),
            usdc_usd: one("kraken", 1.0),
            static_usd: 1.14,
        };
        let mut engine = FairValueEngine::new(FairValueConfig::default());
        let fair = engine.compose(legs, Duration::from_secs(5), Default::default());

        let rows = leg_samples(7, "EURC", &legs, STALE, BAND, &fair);
        let basis = rows
            .iter()
            .find(|r| r.leg == LEG_CRYPTO_USDC)
            .expect("the basis leg resolved");

        // The two numbers are both present and both meaningful: the fast
        // median that guards dislocations, and the fused estimate the
        // composition actually priced off.
        assert_eq!(basis.value, 1.020, "the fast consensus median");
        let fused = basis.fused_value.unwrap();
        assert!(
            (1.020..=1.021).contains(&fused),
            "the stray print did not drag it: {fused}"
        );
        assert_eq!(basis.fusion_step.as_deref(), Some("Seeded"));
        assert_eq!(basis.contributor_count, 3, "three corroborated the median");
        assert_eq!(basis.fused_count, Some(2), "but only two were fused");
        assert!(basis.fused_sigma.unwrap() > 0.0);

        // The peg leg is not fused at all, and says so with NULLs rather than
        // borrowing another leg's numbers.
        let peg = rows.iter().find(|r| r.leg == LEG_USDC_USD).unwrap();
        assert_eq!(peg.fused_value, None);
        assert_eq!(peg.fusion_step, None);
        assert_eq!(peg.fused_count, None);

        // Every source that answered a fused leg is attributed, the trimmed one
        // included — that row is the record of what the estimator refused.
        let rows = contribution_samples(7, "EURC", &fair);
        let stray = rows
            .iter()
            .find(|r| r.source == "coingecko")
            .expect("the trimmed source is still written");
        assert_eq!(stray.weight, 0.0);
        assert_eq!(stray.value, 0.530);
        assert_eq!(stray.leg, LEG_CRYPTO_USDC);
        assert!(
            rows.iter().all(|r| r.leg != LEG_USDC_USD),
            "the peg leg is never fused"
        );
        assert!(
            rows.iter().filter(|r| r.weight > 0.0).count() == 3,
            "one FX source and the two credible venues"
        );
    }

    /// The `fusion_step` column's rendered text is a wire contract: the
    /// migration tells readers to match the dislocation case with
    /// `LIKE 'Reseeded%'`, because it is the one value carrying a payload.
    /// Pinned here because renaming the variant or its field would silently
    /// break every dashboard and alert predicate keyed on it, with nothing
    /// else in the suite failing.
    #[test]
    fn the_reseeded_step_renders_with_its_payload_and_a_stable_prefix() {
        use dropset_fair_value::{FairValueConfig, FairValueEngine};

        let mut engine = FairValueEngine::new(FairValueConfig::default());
        let pair = |a: f64, b: f64| Legs {
            fx: one("oanda", a).push("twelvedata", Some(Reading::new(b, Duration::ZERO))),
            crypto_usdc: one("coinbase", 1.0),
            usdc_usd: one("kraken", 1.0),
            static_usd: 1.14,
        };

        // Seed, then step the tape far enough to trip the innovation gate.
        engine.compose(
            pair(1.1400, 1.1401),
            Duration::from_secs(5),
            Default::default(),
        );
        let legs = pair(1.1600, 1.1601);
        let fair = engine.compose(legs, Duration::from_secs(5), Default::default());

        let rows = leg_samples(9, "EURC", &legs, STALE, BAND, &fair);
        let fx = rows.iter().find(|r| r.leg == LEG_FX).unwrap();
        let step = fx.fusion_step.as_deref().expect("the fx leg fused");
        assert!(
            step.starts_with("Reseeded"),
            "the alert-keyed prefix must be stable: {step}"
        );
        assert!(
            step.contains("innovation_frac"),
            "and it must carry the dislocation size: {step}"
        );
        assert_ne!(step, "Reseeded", "equality matching must not work on it");
    }

    /// A leg whose only candidate is too stale to use resolves to nothing, so
    /// it writes no row. The staleness signal for that source lives in
    /// `feed_health`, which is per source rather than per leg.
    #[test]
    fn a_leg_whose_candidates_are_all_stale_writes_no_row() {
        let legs = Legs {
            fx: Candidates::none().push(
                "frankfurter",
                Some(Reading::new(1.14, Duration::from_secs(3_600))),
            ),
            crypto_usdc: Candidates::none(),
            usdc_usd: Candidates::none(),
            static_usd: 1.14,
        };
        assert!(leg_samples(1, "EURC", &legs, STALE, BAND, &fair(None)).is_empty());
    }

    #[test]
    fn a_disabled_handle_swallows_everything_and_counts_nothing() {
        let telemetry = Telemetry::disabled();
        telemetry.emit(Record::Legs(vec![]));
        // A disabled handle is not a full channel — nothing was offered, so
        // nothing was dropped.
        assert_eq!(telemetry.dropped(), 0);
        assert!(telemetry.health_reporter().is_none());
    }

    #[test]
    fn a_full_channel_drops_and_counts_rather_than_blocking() {
        let (tx, _rx) = mpsc::channel::<Record>(1);
        let telemetry = Telemetry {
            tx: Some(tx),
            dropped: Arc::new(AtomicU64::new(0)),
        };

        for _ in 0..4 {
            telemetry.emit(Record::Legs(vec![]));
        }
        // The tick loop is synchronous; it must never wait on the drain.
        assert_eq!(telemetry.dropped(), 3);
    }
}
