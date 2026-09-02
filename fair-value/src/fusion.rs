//! The scalar fusion estimator — one leg's healthy sources combined into the
//! number the system believes (§1 fair-price estimation).
//!
//! The consensus filter answers *"do these sources agree?"* and summarizes them
//! robustly with a median. That is the right shape for a guard and the wrong
//! shape for an estimate: a median discards every source but the middle one, so
//! three sources of very different fidelity contribute exactly as much as three
//! copies of the best one, and a source that publishes its own uncertainty has
//! nowhere to put it. This module is the estimator half — a **scalar
//! Kalman-family filter** that fuses every healthy candidate at once, weighting
//! each by how much it is trusted to know.
//!
//! The design premise, from the spec: none of the free feeds is a high-rate FX
//! feed, and that is acceptable *because* many low-fidelity sources fuse into
//! one decent estimate. Fusion is what buys the fidelity, not any single source.
//! Explicitly **no production ML** — adaptive weighting and regime-switching
//! estimators are a separate, later piece of work.
//!
//! # Two signals, not one
//!
//! A filter of this family lags a step change, which is exactly the regime where
//! a maker gets picked off. So the leg carries two numbers with different jobs:
//!
//! - the **fast consensus median** over the tape-class sources, which moves
//!   immediately when they agree on a step (see [`crate::Candidates::resolve`]);
//! - the **fused estimate** here, which is smoother, uses every source, and
//!   carries a variance.
//!
//! The quote is composed from the fused estimate, and an **innovation gate**
//! reconciles the two: when the fast median departs from the fused estimate by
//! more than the gate allows, that is a dislocation the tape agrees on rather
//! than noise, and the filter **re-seeds to the median** instead of crawling
//! into it. See [`Fusion::update`].
//!
//! The two rejected shapes, recorded so the choice is not silently revisited:
//! quoting off the median and keeping the fused estimate for analytics only
//! leaves the estimator out of the thing it exists to price; and emitting both
//! for the kill-switch policy to choose between pushes a *pricing* decision into
//! a layer that today only gates width and halts, which is a worse place for it.
//!
//! # Publication conventions do not pool
//!
//! Sources are classed ([`crate::SourceClass`]) by how they publish, and the two
//! classes are used differently on purpose. A daily central-bank fix is
//! authoritative for the moment it names and says nothing about the last six
//! hours; a minute tape is the opposite. Pooling them into one median lets a
//! stale fix drag the fast signal, which is the standing rule against mixing
//! publication conventions.
//!
//! So a reference-class source is kept **out of the fast median** and **in the
//! fusion**, entering as a timestamped, wide-variance measurement — which is
//! precisely what a filter of this family is built to accept. Its variance is
//! inflated by its own age (see [`Fusion::measurement_variance`]), so a fix
//! published this morning is worth much less by evening without anyone having to
//! decide when it stops counting.
//!
//! An official rate that disagrees with the tape is signal, not noise: the
//! disagreement is reported per source in the [`FusedContribution`] set rather than
//! quietly averaged away.
//!
//! # Growing to N heterogeneous sources
//!
//! Source count per leg is a growing variable, not a fixed three, so nothing
//! here is written for the current roster. The batch update below is in
//! **information form**, which is the shape that degrades gracefully: each
//! source contributes `1/R_i` independently, so adding a source is addition and
//! removing one is its absence — there is no per-count special case and no
//! weight vector to renormalize. The three counts that would otherwise be
//! special cases fall out of it:
//!
//! - **N = 0** — nothing to fuse. The estimate is carried, its variance grown by
//!   the elapsed time. The leg is absent and the composition handles it.
//! - **N = 1** — an explicit pass-through with the source's own variance rather
//!   than a degenerate one-source filter. See [`Fusion::update`]: with no prior
//!   there is nothing to fuse *with*, and pretending otherwise would report a
//!   confidence the single reading does not carry.
//! - **N ≥ 2** — the ordinary batch update.
//!
//! # Absorbing a source once per publication, not once per tick
//!
//! A recursive filter is only entitled to add a measurement's precision to its
//! prior when that measurement is **new evidence**. The prior already contains
//! everything absorbed before it, so re-absorbing an unchanged reading counts
//! one observation twice — and on a tick loop far faster than the source
//! publishes, thousands of times. A standard update treats each count as
//! independent evidence and converges the *variance* accordingly.
//!
//! The consequence is confined to the variance, not the value — the estimate
//! converges to the reading either way — but it is the variance that
//! [`FusionReport::sigma`] advertises as a spread-width input.
//!
//! ## The wrong quantity is the observation interval
//!
//! For a source publishing every `T` seconds, read at variance `R`, under
//! process noise `q`, the steady-state posterior variance is `sqrt(q * T * R)`.
//! A filter that absorbs on every tick computes `sqrt(q * dt * R)` instead, so
//! its sigma is too tight by exactly `(T / dt)^(1/4)`. For a daily fix on a 5 s
//! tick that is roughly **11x**. The defect is not "slow sources" in general —
//! it is the single substitution of `dt` for `T`.
//!
//! So the filter must absorb each source once per `T`, and there are only two
//! ways to know when that is: **observe** the publication, or **be told** the
//! interval.
//!
//! ## Why the interval is configured rather than observed
//!
//! Observing would need each reading's own publication instant, and no such
//! instant reaches this crate for the class that needs it. [`crate::Reading`]
//! carries an `age`, and for the reference roster that age is stamped at
//! **receipt**: the maker-bot's cache re-stamps every drain, and the Frankfurter
//! source re-emits the ECB fix on every poll, so a fix published this morning
//! still arrives claiming to be a second old. Deriving a publication instant
//! from that age would mark every tick as a fresh publication and change
//! nothing.
//!
//! That upstream receipt-stamping is a defect in its own right — it is equally
//! why the age inflation below cannot bite on a reference source, and why the
//! bot suppresses Frankfurter over the weekend by hand rather than letting it
//! age out. Repairing it is a feeds-and-transport change, not this filter's,
//! and it would let [`FusionConfig::reference_publish_interval`] be replaced by
//! the observed instant.
//!
//! Being told the interval gives up only **phase**: the filter absorbs once per
//! `T` from an arbitrary offset rather than at the true publication moment.
//! Phase moves *when* the variance resets, not how wide it gets between resets,
//! and it is the width a spread model consumes.
//!
//! ## What this does to the reported sigma
//!
//! On a reference-only leg the sigma now traces a **sawtooth** rather than
//! sitting flat: tight just after an absorption, widening on drift alone until
//! the next one. That is the honest shape — with a daily fix as the only evidence,
//! uncertainty about *now* genuinely grows through the day and resets at the
//! fix. A leg with a live tape alongside is unaffected, because the tape
//! dominates the precision and re-absorbs legitimately.
//!
//! ## A warm start must carry the absorption clocks
//!
//! The filter's state is no longer just an estimate and a variance — it is
//! those plus how long ago each source was absorbed. Restoring the first two
//! without the third would present every source as never-absorbed, so the next
//! tick would take them all in at once and hand back exactly the
//! over-confidence this mechanism removes, on a posterior that then defends
//! itself against the live tape through the variance-aware re-seed gate.
//!
//! The safe direction is the conservative one: a restored clock that is *too
//! old* only re-absorbs a source earlier than it should, which is the error
//! this filter already tolerates. So a warm start that cannot recover the
//! clocks should restore none of the state rather than part of it.
//!
//! ## What is deliberately left alone
//!
//! **Tape-class sources are not throttled.** A tape polled at roughly its own
//! cadence really does bring a fresh observation each poll, which is the case
//! this whole mechanism exists to distinguish from a repeated one. A tape
//! polled *much* faster than it prints has a milder form of the same
//! over-count — bounded by the same fourth-root, so a 10x over-poll is ~1.8x
//! rather than 11x — and no throttle is applied for it, because the interval
//! that would fix it is not known per source and a wrong one costs more than
//! the error it corrects. Detecting an unchanged value would suppress a
//! flat-but-live tape on a quiet market, trading over-confidence for
//! under-confidence rather than removing it.

use std::time::Duration;

use crate::config::ConfigError;
use crate::consensus::{Candidate, SourceClass, MAX_CANDIDATES};

/// Tuning for the fusion filter. Grouped rather than flattened into
/// [`crate::FairValueConfig`] because the five constants are only meaningful
/// together — a noise scale means nothing without the drift rate it is measured
/// against.
///
/// **Every value is TBD(analytics)**, like the rest of the calibration surface.
#[derive(Clone, Copy, Debug)]
pub struct FusionConfig {
    /// How far the underlying truth may wander in one second, as a fraction of
    /// its own value — the random-walk process noise.
    ///
    /// This one constant does two jobs, which is why it is a rate rather than a
    /// per-tick variance: it grows the estimate's variance between updates, and
    /// it inflates a measurement's variance by that measurement's *age*. Both
    /// ask the same question — how much can the truth have moved in this much
    /// time — so answering it twice with two knobs would let them disagree.
    pub drift_frac_per_sec: f64,

    /// Measurement noise for a [`SourceClass::Tape`] source that publishes no
    /// confidence of its own, as a fraction of the reading.
    pub tape_noise_frac: f64,

    /// Measurement noise for a [`SourceClass::Reference`] source, as a fraction
    /// of the reading — before the age inflation above, which is what actually
    /// makes a daily fix cheap by evening.
    ///
    /// Wider than the tape figure even at zero age: a fix is a single daily
    /// snapshot of a market that trades continuously, so it is a coarser
    /// measurement of *now* even at the instant it is published.
    pub reference_noise_frac: f64,

    /// How many standard deviations the fast median may sit from the fused
    /// estimate before the departure is read as a dislocation and the filter
    /// re-seeds.
    ///
    /// Not a fraction — it is a multiple of the estimate's own uncertainty, so
    /// a well-corroborated estimate defends itself harder than a shaky one,
    /// which is the whole reason the filter carries a variance at all.
    pub reseed_sigma: f64,

    /// Floor under the re-seed gate, as a fraction of the estimate.
    ///
    /// Load-bearing, not belt-and-braces. A filter fed several agreeing sources
    /// drives its variance small, and a small variance makes a σ-multiple gate
    /// small in absolute terms — so without a floor the estimator would re-seed
    /// on ordinary tick-to-tick noise precisely when it was working best,
    /// which converts the filter into a slower spelling of the median.
    pub reseed_floor_frac: f64,

    /// How often a [`SourceClass::Reference`] source publishes a genuinely new
    /// observation — the `T` the module header derives.
    ///
    /// The filter absorbs a reference source's precision at most once per this
    /// interval and carries it in between, because the alternative is counting
    /// one daily fix as thousands of independent observations. It is a
    /// **configured** interval rather than an observed one for the reason the
    /// header gives: no publication instant survives the transport, so the
    /// filter cannot see when a fix actually moved.
    ///
    /// Set it to the source's real cadence. Setting it *too short* restores the
    /// over-confidence this exists to remove; setting it too long only makes
    /// the estimate conservative, so the two errors are not symmetric and the
    /// safe direction is long.
    ///
    /// No tape counterpart, deliberately — see the header's closing section for
    /// why a tape is left alone.
    pub reference_publish_interval: Duration,
}

impl FusionConfig {
    /// Check the invariants the filter relies on but cannot enforce.
    ///
    /// Every one of these would fail *quietly* rather than loudly. A
    /// non-positive noise fraction divides by zero into an infinite precision,
    /// so one source would win every fusion outright and the estimate would be
    /// that source with extra steps; a non-positive `reseed_sigma` makes the
    /// gate the floor alone, silently discarding the variance-aware half of the
    /// dislocation test. None of it is reachable from the compile-time markets
    /// table — this is hardening for the later runtime-config path, on the same
    /// reasoning as [`crate::FairValueConfig::validate`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (value, field) in [
            (self.drift_frac_per_sec, "fusion.drift_frac_per_sec"),
            (self.tape_noise_frac, "fusion.tape_noise_frac"),
            (self.reference_noise_frac, "fusion.reference_noise_frac"),
            (self.reseed_floor_frac, "fusion.reseed_floor_frac"),
        ] {
            if !value.is_finite() || value <= 0.0 || value > 1.0 {
                return Err(ConfigError::NotAFraction(field));
            }
        }
        // Not a fraction: it is a multiple of a standard deviation, and the
        // useful settings are all above 1.
        if !self.reseed_sigma.is_finite() || self.reseed_sigma <= 0.0 {
            return Err(ConfigError::NotPositive("fusion.reseed_sigma"));
        }
        // A zero interval is not a neutral setting — it is precisely the
        // absorb-every-tick behavior the throttle exists to remove, and it
        // would fail silently as an over-confident sigma rather than loudly.
        if self.reference_publish_interval.is_zero() {
            return Err(ConfigError::NotPositive(
                "fusion.reference_publish_interval",
            ));
        }
        Ok(())
    }
}

impl Default for FusionConfig {
    fn default() -> Self {
        Self {
            // Placeholder: 1 bp per second of drift. Over a 60 s gap that is
            // ~7.7 bp of added uncertainty, and it prices a six-hour-old daily
            // fix at percent-scale noise — which is the intended reading of a
            // fix that old. TBD(analytics): from the observed FX increment
            // distribution.
            drift_frac_per_sec: 1e-4,
            // Placeholder: 5 bp for a venue tape with no published confidence.
            // TBD(analytics): per source, from the cross-venue residuals.
            tape_noise_frac: 5e-4,
            // Placeholder: 20 bp for a daily reference fix at zero age.
            // TBD(analytics).
            reference_noise_frac: 2e-3,
            // Placeholder: four sigma. Wide enough that ordinary disagreement
            // between healthy sources does not re-seed, narrow enough that a
            // genuine step the tape agrees on is adopted on the tick it
            // happens. TBD(analytics).
            reseed_sigma: 4.0,
            // Placeholder: 20 bp. TBD(analytics).
            reseed_floor_frac: 2e-3,
            // The ECB reference fix — the only reference-class source on the
            // roster today — publishes once a business day, so one day is the
            // honest default rather than a placeholder. TBD(analytics) only in
            // the sense that a second reference source on a different cadence
            // would need this to become per-source rather than per-class.
            reference_publish_interval: Duration::from_secs(86_400),
        }
    }
}

/// One source's share of a **fused estimate**.
///
/// # Not [`crate::Contributor`], and the difference matters
///
/// The resolver has its own per-source attribution, and this is deliberately a
/// second one rather than a replacement. They describe two different mechanisms
/// over the same leg-tick:
///
/// - [`crate::Contributor`] attributes the **fast consensus**. Every resolution
///   is a linear combination of its members' values, so those weights are
///   *exact* — a median of three credits its middle member `1.0`, an agreeing
///   pair credits each `0.5`, a designated source anchors at `1.0`.
/// - `FusedContribution` attributes the **estimate**, which is what the
///   composition actually prices off. Its weights are shares of the posterior
///   *information*, so they answer "how much did this source's precision move
///   the number", a question the consensus weights cannot express because the
///   median does not weigh precision at all.
///
/// The two legitimately disagree on the same tick, and neither is derivable
/// from the other. A source outside the middle of a median carries no consensus
/// weight yet may be fused at full precision; a reference fix carries no
/// consensus weight *by construction* (it is not in the fast set) yet informs
/// the estimate; and a trimmed source appears here at zero while still being
/// counted in [`crate::Consensus::n`].
///
/// `weight` sums to strictly less than 1 whenever a prior estimate also
/// contributed, and the shortfall *is* the prior's share — which is why the
/// prior is not listed as a contributor. It is not a source, and naming it
/// would put a row in an attribution set that no feed is accountable for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FusedContribution {
    /// The source that contributed, as its stable identifier. Same vocabulary
    /// as [`crate::Contributor::source`] — the bare venue name, joined to the
    /// per-feed health table on the `:` prefix rather than by equality.
    pub source: &'static str,
    /// What it read.
    pub value: f64,
    /// The measurement variance it was fused at — its published confidence
    /// where it has one, else its class noise, inflated by its age.
    ///
    /// No counterpart on [`crate::Contributor`], and it is the field that makes
    /// this set an *estimate's* attribution rather than a *combination's*: the
    /// weight is a consequence of this number, so recording only the weight
    /// would leave the reason for it unrecoverable.
    pub variance: f64,
    /// Its share of the posterior information, in `[0, 1]`.
    ///
    /// **Zero has two causes, and they are not distinguishable from this set.**
    /// Either the source was **trimmed** — it sat outside the dispersion band
    /// of the fast consensus, so the estimator declined to believe it — or it
    /// was **already absorbed**, a throttled reference source between
    /// publications (see the module header). The first is a disagreement worth
    /// an operator's attention; the second is the routine steady state of a
    /// reference-only leg, and on such a leg it is the *common* case.
    ///
    /// Reading them apart needs a discriminator this struct does not carry —
    /// deliberately, since the persisted attribution row would need one too and
    /// that is a schema change rather than a filter change.
    pub weight: f64,
}

/// What one [`Fusion::update`] did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FusionStep {
    /// Nothing was fused this tick. The estimate was carried unchanged and its
    /// variance grown by the elapsed time.
    ///
    /// **Four conditions reach this, and only one of them is a feed outage.**
    /// A reader who assumes the first will misdiagnose the others:
    ///
    /// - no healthy source answered at all;
    /// - sources answered and **every one was trimmed** — either they all sat
    ///   outside the dispersion band, or the leg had no fast consensus to trim
    ///   against (an absent leg, or a dispersed pair). This is the case to
    ///   watch: it appears beside a *non-zero* contributor count;
    /// - sources answered and **every one had already been absorbed** — the
    ///   throttled steady state of a reference-only leg between publications,
    ///   described in the module header. This is the *routine* case, not a
    ///   fault: the estimate is resting on an observation it already holds
    ///   while its variance widens on drift;
    /// - the accumulated precision came out non-finite, so the update was
    ///   declined rather than applied.
    ///
    /// So `Carried` alongside contributors means the estimator did not take in
    /// what it was offered — because it disagreed with it, or because it
    /// already had it — which is a very different operator story from silence.
    Carried,
    /// The filter had no prior, so this tick established one outright.
    ///
    /// **Not the same as "one source".** This is reached whenever the estimate
    /// is `None` — a leg's first tick at *any* source count, and every recovery
    /// from a filter that never seeded. With one source it is the documented
    /// N = 1 pass-through at that source's own variance; with several they
    /// corroborate each other and combine normally. Read the count from
    /// [`FusionReport::n`], never from this variant.
    Seeded,
    /// The ordinary batch update.
    Fused,
    /// The fast median departed from the estimate by more than the gate
    /// allowed, so the estimate was re-seeded to it rather than smoothed
    /// toward it. Carries the size of the departure as a fraction of the
    /// estimate, so an operator can see how big the dislocation was.
    Reseeded { innovation_frac: f64 },
}

impl FusionStep {
    /// Whether this step adopted a dislocation the tape agreed on.
    pub fn reseeded(self) -> bool {
        matches!(self, Self::Reseeded { .. })
    }
}

/// One leg's fused estimate for one tick, with everything an operator needs to
/// see how it was reached.
#[derive(Clone, Copy, Debug)]
pub struct FusionReport {
    /// The fused value, or `None` while the filter has never been seeded.
    pub value: Option<f64>,
    /// The estimate's variance. Its square root is a standard deviation in the
    /// leg's own units, which is what a spread-width model wants.
    pub variance: f64,
    /// What this update did.
    pub step: FusionStep,
    /// How many sources were fused this tick.
    pub n: usize,
    /// Each contributing source's share.
    ///
    /// Private, with [`FusionReport::contributions`] as the only way in — so
    /// the `None` padding stays an implementation detail rather than part of
    /// the public surface. Matches [`crate::Consensus::healthy`], which made
    /// the same choice for the same reason.
    contributions: [Option<FusedContribution>; MAX_CANDIDATES],
}

impl FusionReport {
    /// The report of a leg that was never fused — no estimate, no contributors,
    /// infinite variance.
    ///
    /// The variance is `INFINITY` rather than zero on purpose: variance is
    /// *un*certainty, so the neutral value for "nothing is known" is unbounded.
    /// A zero would claim perfect confidence in an estimate that does not
    /// exist, and any consumer dividing by it or comparing against a threshold
    /// would read the absence as the strongest possible signal.
    pub fn none() -> Self {
        Self {
            value: None,
            variance: f64::INFINITY,
            step: FusionStep::Carried,
            n: 0,
            contributions: [None; MAX_CANDIDATES],
        }
    }

    /// The standard deviation of the estimate, when there is one.
    pub fn sigma(&self) -> Option<f64> {
        self.value.is_some().then(|| self.variance.sqrt())
    }

    /// Every contributing source's share, in fusion order.
    pub fn contributions(&self) -> impl Iterator<Item = &FusedContribution> {
        self.contributions.iter().flatten()
    }
}

/// The scalar fusion filter for one leg. One instance per leg per market.
///
/// Deliberately **not** `Copy`: it is a mutating accumulator, so a silent
/// copy-on-assign would fork its history and leave two filters diverging under
/// one market's name.
///
/// Note this diverges from `basis::BasisEma`, which *is* `Copy`
/// despite being an accumulator too — so the two are inconsistent, and this is
/// the side that is right. `FairValueEngine` is itself not `Copy` for exactly
/// this reason, which is what has kept the EMA's `Copy` from causing trouble:
/// nothing today copies one out of the engine that owns it.
#[derive(Clone, Debug)]
pub struct Fusion {
    /// Current estimate; `None` until the first measurement seeds it.
    estimate: Option<f64>,
    /// Current variance of the estimate, in squared leg units. Meaningless
    /// while `estimate` is `None`.
    variance: f64,
    /// How long ago each source's reading was last absorbed — the state that
    /// makes a throttled source's precision count once per publication rather
    /// than once per tick.
    ///
    /// Elapsed time per source rather than an absolute clock: the filter is
    /// only ever asked "has `T` passed for this source", never "when was it",
    /// so carrying an origin would be state with no reader — and an f64 second
    /// count accumulating for the life of a market is a precision question
    /// nobody needs to answer. `Duration` saturates, which is the right
    /// overflow behavior here: a source absorbed impossibly long ago should
    /// stay eligible, not wrap to freshly-absorbed.
    absorbed: [Option<Absorbed>; MAX_CANDIDATES],
    cfg: FusionConfig,
}

impl Fusion {
    /// A fresh, unseeded filter.
    pub fn new(cfg: FusionConfig) -> Self {
        Self {
            estimate: None,
            variance: f64::INFINITY,
            absorbed: [None; MAX_CANDIDATES],
            cfg,
        }
    }

    /// How long ago `source` was last absorbed, or `None` if it never has been.
    fn absorbed_since(&self, source: &str) -> Option<Duration> {
        self.absorbed
            .iter()
            .flatten()
            .find(|a| a.source == source)
            .map(|a| a.since)
    }

    /// Age every source's absorption clock by the elapsed tick.
    fn age_absorbed(&mut self, dt: Duration) {
        for a in self.absorbed.iter_mut().flatten() {
            a.since = a.since.saturating_add(dt);
        }
    }

    /// Record that `source`'s reading was taken into the estimate just now.
    ///
    /// A full table evicts the entry that has waited **longest**, which is the
    /// entry closest to being eligible anyway — so the eviction costs the least
    /// possible precision, and it errs toward re-absorbing rather than toward
    /// suppressing a source the filter has forgotten. The table only fills if
    /// more distinct sources appear across ticks than a leg can offer in one,
    /// which a stable roster never does.
    fn mark_absorbed(&mut self, source: &'static str) {
        if let Some(slot) = self
            .absorbed
            .iter_mut()
            .flatten()
            .find(|a| a.source == source)
        {
            slot.since = Duration::ZERO;
            return;
        }
        if let Some(empty) = self.absorbed.iter_mut().find(|a| a.is_none()) {
            *empty = Some(Absorbed {
                source,
                since: Duration::ZERO,
            });
            return;
        }
        if let Some(stalest) = self.absorbed.iter_mut().flatten().max_by_key(|a| a.since) {
            *stalest = Absorbed {
                source,
                since: Duration::ZERO,
            };
        }
    }

    /// Reset the absorption clock of every measurement this update took in.
    ///
    /// Called only where an update **commits** — never on the declined-update
    /// paths, because a measurement the filter refused to apply has not been
    /// absorbed and must stay eligible for the next tick. A trimmed reading is
    /// likewise never marked: it may be admitted later, when the consensus it
    /// disagreed with has moved, and it is still the same unabsorbed
    /// observation when that happens.
    fn mark_fused(&mut self, measurements: &[Option<Measurement>]) {
        for m in measurements.iter().flatten().filter(|m| m.fusible()) {
            self.mark_absorbed(m.candidate.source);
        }
    }

    /// Whether this candidate brings a new observation, or is one the filter
    /// has already taken in.
    ///
    /// Only [`SourceClass::Reference`] is throttled — see the module header for
    /// why a tape is left alone. A source never absorbed is always new, which
    /// is what lets a leg seed on its first tick whatever its class.
    fn is_new_observation(&self, c: &Candidate) -> bool {
        if !matches!(c.class, SourceClass::Reference) {
            return true;
        }
        self.absorbed_since(c.source)
            .is_none_or(|since| since >= self.cfg.reference_publish_interval)
    }

    /// Fuse this tick's healthy candidates into the estimate.
    ///
    /// `healthy` is the leg's freshness-filtered candidate set (tape and
    /// reference class alike — the class decides the variance, not the
    /// membership). `fast` is the fast consensus median over the tape-class
    /// sources, which drives the dislocation gate; `None` when the leg has no
    /// fast signal this tick, in which case there is nothing to gate against and
    /// the ordinary update runs.
    ///
    /// `dt` is the elapsed time since the previous update, which grows the
    /// estimate's variance — so a filter that has not been fed for a while
    /// weights a returning measurement more, exactly as the basis EMA's decay
    /// does, and for the same reason.
    ///
    /// The order is: grow the variance for elapsed time, test the fast median
    /// against the grown estimate, then either re-seed or fuse. Testing before
    /// growing would judge a returning measurement against a confidence the
    /// estimate no longer has, and re-seed far too eagerly after any gap.
    pub fn update(
        &mut self,
        healthy: &[Option<Candidate>],
        fast: Option<f64>,
        band: f64,
        dt: Duration,
    ) -> FusionReport {
        self.predict(dt);
        self.age_absorbed(dt);

        // The fill zips against the destination array rather than indexing by a
        // running counter, so an oversized `healthy` drops its trailing
        // candidates instead of panicking. `update` is public and takes an
        // arbitrary slice, so the length is not this function's to assume — and
        // the counter form is exactly the pattern `Candidates::resolve` was
        // rewritten away from in this same change.
        let mut measurements = [None; MAX_CANDIDATES];
        let mut n = 0;
        for (slot, c) in measurements.iter_mut().zip(healthy.iter().flatten()) {
            let Some(variance) = self.measurement_variance(c) else {
                continue;
            };
            // Two independent reasons a reading may not be fused, kept apart
            // because they are different facts about it: the trim is *this
            // reading disagrees*, the freshness test is *the filter already
            // holds this observation*. Only a reading that is both admitted and
            // new contributes precision; either alone reports at weight zero.
            let admitted = admits(fast, band, c.reading.value);
            let fresh = self.is_new_observation(c);
            *slot = Some(Measurement {
                candidate: *c,
                variance,
                admitted,
                fresh,
            });
            n += usize::from(admitted && fresh);
        }
        let measurements = &measurements[..];

        let Some(prior) = self.estimate else {
            return self.seed(measurements, n);
        };

        if n == 0 {
            return FusionReport {
                value: Some(prior),
                variance: self.variance,
                step: FusionStep::Carried,
                n: 0,
                contributions: attribute(measurements, 0.0),
            };
        }

        // The dislocation gate. `fast` is the tape's own immediate view, so a
        // departure this large is the sources *agreeing* on a step rather than
        // one of them misprinting — which is the case a smoothing filter handles
        // worst and a maker pays for most.
        if let Some(fast) = fast {
            let innovation = fast - prior;
            if prior.is_finite() && prior > 0.0 && innovation.abs() > self.reseed_gate(prior) {
                return self.reseed(fast, innovation / prior, measurements, n);
            }
        }

        self.fuse(prior, measurements, n)
    }

    /// Grow the estimate's variance for `dt` of elapsed time — the random-walk
    /// prediction step. A degenerate `dt` adds nothing rather than poisoning the
    /// variance with a NaN.
    fn predict(&mut self, dt: Duration) {
        let Some(estimate) = self.estimate else {
            return;
        };
        let secs = dt.as_secs_f64();
        if !secs.is_finite() || secs <= 0.0 {
            return;
        }
        let drift = self.cfg.drift_frac_per_sec * estimate;
        self.variance += drift * drift * secs;
    }

    /// The variance to fuse one candidate at: its published confidence where it
    /// has one, else its class noise — either way inflated by the candidate's
    /// own age, because both are statements about the moment the source
    /// published and the estimate is about now.
    ///
    /// The age term is `drift_frac_per_sec * value * sqrt(age)`, the same random
    /// walk [`Fusion::predict`] integrates over elapsed time. See the note in
    /// the body for why the square root is mandatory rather than cosmetic.
    ///
    /// `None` for a candidate that cannot be fused at all (a non-finite or
    /// non-positive reading, or a variance that does not come out finite and
    /// positive). Such a candidate is skipped rather than defaulted: a
    /// measurement whose uncertainty cannot be established has no business
    /// moving an estimate, and giving it a made-up variance would be exactly the
    /// fabricated-parity failure the basis path already refuses.
    fn measurement_variance(&self, c: &Candidate) -> Option<f64> {
        let value = c.reading.value;
        if !value.is_finite() || value <= 0.0 {
            return None;
        }

        // A published confidence is a half-width the source stands behind, so it
        // is preferred over any class default — that is the whole value of a
        // source that publishes one.
        let base_sigma = match c.reading.confidence {
            Some(conf) if conf.is_finite() && conf > 0.0 => conf,
            _ => {
                let frac = match c.class {
                    SourceClass::Tape => self.cfg.tape_noise_frac,
                    SourceClass::Reference => self.cfg.reference_noise_frac,
                };
                frac * value
            }
        };

        // Age inflation is **sqrt(age)** in sigma, i.e. linear in variance, and
        // that is forced rather than chosen: it is the same random walk
        // `predict` integrates, so the two must scale together or the single
        // `drift_frac_per_sec` that drives both would mean two different things
        // depending on which side read it.
        //
        // Getting this wrong is not a rounding matter. Linear-in-sigma inflation
        // squares to variance proportional to age^2, which at a six-hour-old
        // daily fix overstates the variance by ~21,600x — so the fix's weight,
        // being one over that, all but vanishes. The source would then be
        // reported as fused while contributing nothing, which is precisely the
        // outcome the reference class exists to avoid.
        let age = c.reading.age.as_secs_f64();
        let staleness_sigma = if age.is_finite() && age > 0.0 {
            self.cfg.drift_frac_per_sec * value * age.sqrt()
        } else {
            0.0
        };

        let variance = base_sigma * base_sigma + staleness_sigma * staleness_sigma;
        (variance.is_finite() && variance > 0.0).then_some(variance)
    }

    /// How far the fast median may sit from the estimate before the departure
    /// reads as a dislocation: `reseed_sigma` standard deviations, floored at
    /// `reseed_floor_frac` of the estimate. See
    /// [`FusionConfig::reseed_floor_frac`] for why the floor is load-bearing.
    fn reseed_gate(&self, estimate: f64) -> f64 {
        let sigma_gate = self.cfg.reseed_sigma * self.variance.sqrt();
        let floor = self.cfg.reseed_floor_frac * estimate.abs();
        if sigma_gate.is_finite() {
            sigma_gate.max(floor)
        } else {
            floor
        }
    }

    /// Seed the filter from this tick's measurements — the N = 1 path, and also
    /// the first tick of any leg.
    ///
    /// With no prior there is nothing to fuse *with*, so a single measurement is
    /// passed through at its own variance rather than run through the update
    /// equations. The equations would produce the same value, so this is not
    /// about the arithmetic: it is that a degenerate one-source filter reports a
    /// posterior variance as though something had corroborated the reading, when
    /// nothing has. Several measurements on a seeding tick *do* corroborate each
    /// other, so they combine normally.
    fn seed(&mut self, measurements: &[Option<Measurement>], n: usize) -> FusionReport {
        let (information, weighted) = accumulate(measurements, 0.0, 0.0);

        if information <= 0.0 || !information.is_finite() || !weighted.is_finite() {
            return FusionReport {
                value: self.estimate,
                variance: self.variance,
                step: FusionStep::Carried,
                n: 0,
                contributions: attribute(measurements, 0.0),
            };
        }

        let variance = 1.0 / information;
        let value = weighted * variance;
        self.estimate = Some(value);
        self.variance = variance;
        self.mark_fused(measurements);

        FusionReport {
            value: Some(value),
            variance,
            step: FusionStep::Seeded,
            n,
            contributions: attribute(measurements, information),
        }
    }

    /// The ordinary batch update, in information form: the posterior precision
    /// is the prior's plus every measurement's, and the posterior mean is the
    /// precision-weighted mean of all of them.
    ///
    /// Written as a batch rather than as a fold of sequential single-measurement
    /// updates because the two are only equivalent when the measurements are
    /// independent *and* processed against the same prior — a sequential fold
    /// silently lets an early source in the array set the prior the later ones
    /// are judged against, making the result depend on offer order. Offer order
    /// carries no information here, so it must not decide anything.
    fn fuse(&mut self, prior: f64, measurements: &[Option<Measurement>], n: usize) -> FusionReport {
        let prior_information = if self.variance.is_finite() && self.variance > 0.0 {
            1.0 / self.variance
        } else {
            0.0
        };

        let (information, weighted) =
            accumulate(measurements, prior_information, prior_information * prior);

        if information <= 0.0 || !information.is_finite() || !weighted.is_finite() {
            return FusionReport {
                value: Some(prior),
                variance: self.variance,
                step: FusionStep::Carried,
                n: 0,
                contributions: attribute(measurements, 0.0),
            };
        }

        let variance = 1.0 / information;
        let value = weighted * variance;
        self.estimate = Some(value);
        self.variance = variance;
        self.mark_fused(measurements);

        FusionReport {
            value: Some(value),
            variance,
            step: FusionStep::Fused,
            n,
            contributions: attribute(measurements, information),
        }
    }

    /// Adopt a dislocation: take the fast median as the new estimate, at the
    /// variance this tick's measurements support.
    ///
    /// The measurements still set the variance — a re-seed is a statement about
    /// the *value* being wrong, not about the sources being untrustworthy — so
    /// an estimate re-seeded off a well-corroborated set is immediately
    /// confident again, and one re-seeded off a thin set is not.
    fn reseed(
        &mut self,
        fast: f64,
        innovation_frac: f64,
        measurements: &[Option<Measurement>],
        n: usize,
    ) -> FusionReport {
        let (information, _) = accumulate(measurements, 0.0, 0.0);

        let variance = if information > 0.0 && information.is_finite() {
            1.0 / information
        } else {
            self.variance
        };

        self.estimate = Some(fast);
        self.variance = variance;
        self.mark_fused(measurements);

        FusionReport {
            value: Some(fast),
            variance,
            step: FusionStep::Reseeded { innovation_frac },
            n,
            contributions: attribute(measurements, information),
        }
    }
}

/// One healthy candidate prepared for fusion: the variance it would be fused
/// at, whether the trim admitted it, and whether it is an observation the
/// filter has not already taken in.
#[derive(Clone, Copy, Debug)]
struct Measurement {
    candidate: Candidate,
    variance: f64,
    admitted: bool,
    fresh: bool,
}

impl Measurement {
    /// Whether this reading contributes precision to the estimate — admitted by
    /// the trim *and* not already absorbed.
    fn fusible(&self) -> bool {
        self.admitted && self.fresh
    }
}

/// One source's absorption clock — how long since its reading was last taken
/// into the estimate. See [`Fusion::absorbed`].
#[derive(Clone, Copy, Debug)]
struct Absorbed {
    source: &'static str,
    since: Duration,
}

/// Whether a reading is close enough to the fast consensus to be fused, within
/// `band` as a fraction of that consensus.
///
/// **This trim is what keeps the estimator robust, and it is not optional.**
/// A precision-weighted mean is not robust to an outlier — that is precisely
/// what the median was protecting against — so fusing the raw healthy set
/// throws away the robustness the consensus filter exists to provide. Measured
/// on the case that filter was built for (two venues near 1.02 and an aggregate
/// printing 0.53), the untrimmed fusion lands near 0.86: not the bad print, but
/// dragged far enough to leave the sane band and dark the market. The median
/// ignored it entirely.
///
/// So the two mechanisms compose in one order and only one: the consensus
/// filter decides **which sources to believe**, and the fusion decides **how to
/// weight the ones that survive**. Be robust first, then estimate.
///
/// With no fast consensus to measure against there is nothing to trim by, and
/// admitting everything would be exactly the untrimmed fusion above — so
/// nothing is admitted. That case is a leg the composition has already darked
/// (absent, or a dispersed pair with no majority to appeal to), so the estimate
/// is carried and the leg is not priced off regardless.
fn admits(fast: Option<f64>, band: f64, value: f64) -> bool {
    let Some(fast) = fast else {
        return false;
    };
    if !fast.is_finite() || fast <= 0.0 || !band.is_finite() || band <= 0.0 {
        return false;
    }
    ((value - fast) / fast).abs() <= band
}

/// Add every **fusible** measurement's precision and precision-weighted value
/// to the running totals, which start at the prior's contribution.
fn accumulate(
    measurements: &[Option<Measurement>],
    mut information: f64,
    mut weighted: f64,
) -> (f64, f64) {
    for m in measurements.iter().flatten().filter(|m| m.fusible()) {
        information += 1.0 / m.variance;
        weighted += m.candidate.reading.value / m.variance;
    }
    (information, weighted)
}

/// Each measurement's share of `information`, the posterior precision.
///
/// The shares sum to 1 only when the prior contributed nothing (a seed or a
/// re-seed); after an ordinary update the shortfall is the prior's own share,
/// which is deliberately not listed as a contributor — it is not a source, and
/// naming it as one would put a row in the attribution table that no feed is
/// accountable for.
///
/// **A trimmed source is listed at weight zero, never omitted.** Its reading is
/// the most interesting number on the tick — it is the source that disagreed —
/// and dropping it from the attribution would be suppressing exactly the signal
/// an operator needs, leaving a fused value with no record of what it declined
/// to believe. This is what makes an official rate that contradicts the tape
/// visible rather than quietly averaged away: the row is there, its value is
/// there, and its weight is zero.
///
/// **A throttled source is listed the same way**, for the weaker but related
/// reason that it did answer and its reading is what the estimate is resting
/// on between absorptions. The two zeroes are not distinguishable from the
/// attribution alone — see [`FusedContribution::weight`].
fn attribute(
    measurements: &[Option<Measurement>],
    information: f64,
) -> [Option<FusedContribution>; MAX_CANDIDATES] {
    let mut out = [None; MAX_CANDIDATES];
    let usable = information.is_finite() && information > 0.0;
    for (slot, m) in out.iter_mut().zip(measurements.iter().flatten()) {
        *slot = Some(FusedContribution {
            source: m.candidate.source,
            value: m.candidate.reading.value,
            variance: m.variance,
            weight: if usable && m.fusible() {
                (1.0 / m.variance) / information
            } else {
                0.0
            },
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::Reading;

    /// The leg dispersion band the engine trims with, at its placeholder value.
    const BAND: f64 = 0.02;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn set(candidates: &[Candidate]) -> [Option<Candidate>; MAX_CANDIDATES] {
        let mut out = [None; MAX_CANDIDATES];
        for (slot, c) in out.iter_mut().zip(candidates) {
            *slot = Some(*c);
        }
        out
    }

    fn tape(source: &'static str, value: f64) -> Candidate {
        Candidate::new(source, Reading::new(value, Duration::ZERO))
    }

    fn fusion() -> Fusion {
        Fusion::new(FusionConfig::default())
    }

    /// The N = 1 path: a lone source is passed through at its own variance, and
    /// specifically **not** reported as though anything corroborated it.
    #[test]
    fn a_lone_source_passes_through_at_its_own_variance() {
        let mut f = fusion();
        let r = f.update(&set(&[tape("oanda", 1.14)]), Some(1.14), BAND, secs(5));

        assert_eq!(r.step, FusionStep::Seeded);
        assert_eq!(r.value, Some(1.14));
        assert_eq!(r.n, 1);
        // Its own measurement variance, not something narrowed by fusing.
        let expected = FusionConfig::default().tape_noise_frac * 1.14;
        assert!((r.variance - expected * expected).abs() < 1e-18);
        // One contributor holding all of the information.
        assert_eq!(r.contributions().count(), 1);
        assert!((r.contributions().next().unwrap().weight - 1.0).abs() < 1e-12);
    }

    /// Two equally-trusted sources land midway and end up strictly more certain
    /// than either alone — which is the entire premise: fusion buys fidelity no
    /// single free feed has.
    #[test]
    fn two_sources_fuse_to_between_them_and_sharpen_the_estimate() {
        let mut f = fusion();
        let lone = {
            let mut g = fusion();
            g.update(&set(&[tape("a", 1.14)]), Some(1.14), BAND, secs(5))
                .variance
        };

        let r = f.update(
            &set(&[tape("a", 1.140), tape("b", 1.142)]),
            Some(1.141),
            BAND,
            secs(5),
        );

        assert_eq!(r.n, 2);
        let value = r.value.unwrap();
        assert!(1.140 < value && value < 1.142, "between the two: {value}");
        assert!(
            r.variance < lone,
            "fusing two sources sharpens the estimate"
        );
        // Nothing but the two sources contributed, so the shares sum to 1.
        let total: f64 = r.contributions().map(|c| c.weight).sum();
        assert!((total - 1.0).abs() < 1e-12);
    }

    /// A source that publishes a confidence is fused at it, so a tight one pulls
    /// the estimate toward itself and a wide one barely moves it. This is the
    /// "per-source noise reflecting fidelity" the whole estimator is for.
    #[test]
    fn a_confident_source_outweighs_an_uncertain_one() {
        let mut f = fusion();
        let r = f.update(
            &set(&[
                Candidate::new("tight", Reading::with_confidence(1.140, secs(0), 1e-5)),
                Candidate::new("wide", Reading::with_confidence(1.150, secs(0), 1e-2)),
            ]),
            Some(1.145),
            BAND,
            secs(5),
        );

        let value = r.value.unwrap();
        assert!(
            (value - 1.140).abs() < 1e-4,
            "the tight source dominates: {value}"
        );
        let tight = r.contributions().find(|c| c.source == "tight").unwrap();
        let wide = r.contributions().find(|c| c.source == "wide").unwrap();
        assert!(tight.weight > 0.99 && wide.weight < 0.01);
    }

    /// A reference-class source is fused at a wider variance than a tape source
    /// reading the same value at the same age — before any age inflation.
    #[test]
    fn a_reference_source_is_fused_more_loosely_than_a_tape() {
        let mut f = fusion();
        let r = f.update(
            &set(&[
                tape("oanda", 1.14),
                Candidate::reference("frankfurter", Reading::new(1.14, Duration::ZERO)),
            ]),
            Some(1.14),
            BAND,
            secs(5),
        );

        let tape_c = r.contributions().find(|c| c.source == "oanda").unwrap();
        let fix = r
            .contributions()
            .find(|c| c.source == "frankfurter")
            .unwrap();
        assert!(fix.variance > tape_c.variance);
        assert!(
            fix.weight < tape_c.weight,
            "the fix informs the estimate, but less"
        );
        assert!(fix.weight > 0.0, "and it is not discarded either");
    }

    /// Age inflates a measurement's variance, which is what makes a daily fix
    /// cheap by evening without anyone deciding when it stops counting.
    ///
    /// The **scaling** is asserted, not merely the direction. This started life
    /// as `stale > fresh * 10.0`, which a linear-in-sigma bug also satisfied —
    /// and that bug shipped a six-hour fix at ~21,600x its intended variance,
    /// silently discarding the very source the reference class exists to keep.
    /// A floor cannot catch that; a ratio can.
    #[test]
    fn age_inflation_is_a_random_walk_in_variance() {
        let f = fusion();
        let cfg = FusionConfig::default();
        let value = 1.14;
        let age = 6.0 * 3600.0;

        let variance = f
            .measurement_variance(&Candidate::reference(
                "frankfurter",
                Reading::new(value, secs(6 * 3600)),
            ))
            .unwrap();

        // variance = base^2 + (drift * value)^2 * age  — linear in age, which is
        // what makes sigma proportional to sqrt(age) and matches `predict`.
        let base = cfg.reference_noise_frac * value;
        let drift = cfg.drift_frac_per_sec * value;
        let expected = base * base + drift * drift * age;
        assert!(
            (variance / expected - 1.0).abs() < 1e-9,
            "variance must grow linearly in age: got {variance}, want {expected}"
        );

        // And the sigma the config comment quotes: ~1.47% at six hours, which is
        // "percent-scale" as documented — not the 216% a linear sigma gives.
        let sigma = variance.sqrt();
        assert!(
            (0.010..=0.020).contains(&(sigma / value)),
            "a six-hour-old fix is percent-scale, not hundreds of percent: {}",
            sigma / value
        );
    }

    /// The same random walk must drive the prediction step and the age
    /// inflation, or the one constant that feeds both means two things. Growing
    /// an estimate's variance over `t` seconds and measuring a reading aged `t`
    /// seconds must add the identical amount.
    #[test]
    fn prediction_and_age_inflation_agree() {
        let mut f = fusion();
        let value = 1.14;
        let gap = 600;

        // What the PREDICTION path adds over `gap` seconds. Seeded from a source
        // of negligible published noise, so the seeded variance is ~0 and what
        // remains after the carry is the drift term alone.
        let tight = |v: f64| {
            set(&[Candidate::new(
                "tight",
                Reading::with_confidence(v, Duration::ZERO, 1e-12),
            )])
        };
        f.update(&tight(value), Some(value), BAND, secs(0));
        let seeded = f.variance;
        let predicted = f.update(&set(&[]), None, BAND, secs(gap)).variance - seeded;

        // What the MEASUREMENT path adds for a reading of the same age — the
        // same negligible base, so the difference is again the drift term.
        let g = fusion();
        let at = |age| {
            g.measurement_variance(&Candidate::new(
                "tight",
                Reading::with_confidence(value, secs(age), 1e-12),
            ))
            .unwrap()
        };
        let measured = at(gap) - at(0);

        // The two are the same random walk read from opposite ends, so they must
        // add the identical variance. A linear-in-sigma age term fails this by a
        // factor of `gap`.
        assert!(
            (predicted / measured - 1.0).abs() < 1e-9,
            "predict added {predicted}; a reading aged {gap}s carries {measured}"
        );
    }

    /// `reseed_floor_frac` is documented load-bearing, so removing it must break
    /// something. Many tightly-agreeing sources drive the variance small enough
    /// that the sigma gate alone would re-seed on ordinary noise — the floor is
    /// the only thing holding it, so this test fails if the floor is zeroed.
    #[test]
    fn the_reseed_floor_holds_where_the_sigma_gate_would_not() {
        let cfg = FusionConfig::default();
        let level = 1.1400;
        let mut f = fusion();

        // Six very confident sources, so the posterior variance collapses and
        // the sigma gate has a chance of landing under the floor.
        let tight: Vec<Candidate> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|s| Candidate::new(s, Reading::with_confidence(level, Duration::ZERO, 1e-6)))
            .collect();
        f.update(&set(&tight), Some(level), BAND, secs(5));

        // The gate is evaluated AFTER `predict`, so the variance that matters is
        // the seeded one plus one tick of drift — not the one standing now.
        // Reading it before the next update understates the gate by ~600x and
        // was how the first version of this test came to pass for the wrong
        // reason: its nudge sat three orders of magnitude inside both bounds, so
        // neutering the floor changed nothing.
        let drift = cfg.drift_frac_per_sec * level;
        let predicted = f.variance + drift * drift * 5.0;
        let sigma_gate = cfg.reseed_sigma * predicted.sqrt();
        let floor = cfg.reseed_floor_frac * level;
        assert!(
            sigma_gate < floor,
            "precondition: the sigma gate must be the smaller bound ({sigma_gate} vs {floor})"
        );

        // Land the move strictly between the two bounds. The floor is then the
        // only thing refusing it, so zeroing `reseed_floor_frac` flips this
        // assertion — which is what makes the constant's "load-bearing" claim
        // mean something.
        let nudge = level + (sigma_gate + floor) / 2.0;
        let r = f.update(&set(&tight), Some(nudge), BAND, secs(5));
        assert_eq!(
            r.step,
            FusionStep::Fused,
            "the floor must absorb a move the sigma gate alone would adopt"
        );
    }

    /// The trim: a source outside the dispersion band of the fast consensus is
    /// excluded from the estimate but still **attributed**, at weight zero.
    /// Suppressing it would hide the one number an operator most needs.
    #[test]
    fn a_trimmed_source_is_excluded_but_still_reported() {
        let mut f = fusion();
        let r = f.update(
            &set(&[
                tape("coinbase", 1.020),
                tape("kraken", 1.021),
                tape("coingecko", 0.530),
            ]),
            Some(1.021),
            BAND,
            secs(5),
        );

        assert_eq!(r.n, 2, "only the two credible venues were fused");
        let value = r.value.unwrap();
        assert!(
            (1.020..=1.021).contains(&value),
            "the stray print did not drag it: {value}"
        );

        let stray = r.contributions().find(|c| c.source == "coingecko").unwrap();
        assert_eq!(stray.weight, 0.0);
        assert_eq!(stray.value, 0.530, "its reading is still on the record");
        assert_eq!(r.contributions().count(), 3, "all three are attributed");
    }

    /// The dislocation gate: when the fast median steps well away from the
    /// estimate, the filter adopts it outright rather than crawling toward it.
    /// This is the pick-off case a smoothing filter handles worst.
    #[test]
    fn an_agreed_step_reseeds_instead_of_lagging() {
        let mut f = fusion();
        f.update(
            &set(&[tape("a", 1.140), tape("b", 1.141)]),
            Some(1.1405),
            BAND,
            secs(5),
        );

        let r = f.update(
            &set(&[tape("a", 1.160), tape("b", 1.161)]),
            Some(1.1605),
            BAND,
            secs(5),
        );

        assert!(r.step.reseeded(), "a 175 bp step is a dislocation");
        assert_eq!(r.value, Some(1.1605), "adopted, not smoothed toward");
        let FusionStep::Reseeded { innovation_frac } = r.step else {
            unreachable!()
        };
        assert!(innovation_frac > 0.0, "and its size is reported");
    }

    /// An ordinary tick-to-tick wobble must **not** re-seed, or the estimator
    /// is just a slower spelling of the median.
    #[test]
    fn ordinary_movement_does_not_reseed() {
        let mut f = fusion();
        f.update(
            &set(&[tape("a", 1.1400), tape("b", 1.1401)]),
            Some(1.14005),
            BAND,
            secs(5),
        );

        let r = f.update(
            &set(&[tape("a", 1.1402), tape("b", 1.1403)]),
            Some(1.14025),
            BAND,
            secs(5),
        );

        assert_eq!(r.step, FusionStep::Fused);
    }

    /// With no source answering, the estimate is carried and its variance grows
    /// — so a returning measurement weighs more, exactly as the basis EMA's
    /// decay arranges.
    #[test]
    fn an_empty_tick_carries_the_estimate_and_widens_it() {
        let mut f = fusion();
        let seeded = f.update(&set(&[tape("a", 1.14)]), Some(1.14), BAND, secs(5));

        let r = f.update(&set(&[]), None, BAND, secs(600));

        assert_eq!(r.step, FusionStep::Carried);
        assert_eq!(r.value, seeded.value, "unchanged");
        assert!(r.variance > seeded.variance, "but less certain");
        assert_eq!(r.n, 0);
    }

    /// After an ordinary update the sources' shares sum to strictly less than
    /// 1: the shortfall is the prior estimate's own share, which is deliberately
    /// not listed as a contributor because no feed is accountable for it.
    #[test]
    fn the_prior_holds_the_share_the_sources_do_not() {
        let mut f = fusion();
        f.update(&set(&[tape("a", 1.140)]), Some(1.140), BAND, secs(5));
        let r = f.update(&set(&[tape("a", 1.1401)]), Some(1.1401), BAND, secs(5));

        assert_eq!(r.step, FusionStep::Fused);
        let total: f64 = r.contributions().map(|c| c.weight).sum();
        assert!(total > 0.0 && total < 1.0, "the prior keeps the rest");
    }

    /// A reading with no establishable uncertainty is skipped rather than
    /// defaulted — inventing a variance for it would be the fabricated-parity
    /// failure the basis path already refuses.
    #[test]
    fn an_unusable_reading_is_never_fused() {
        let f = fusion();
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                f.measurement_variance(&tape("bad", value)).is_none(),
                "value {value}"
            );
        }
    }

    /// A published confidence of zero is not a claim of perfect certainty — it
    /// is a malformed field, and falls back to the class default rather than
    /// winning every fusion outright by dividing by zero.
    #[test]
    fn a_zero_confidence_falls_back_to_the_class_default() {
        let f = fusion();
        let zeroed = f
            .measurement_variance(&Candidate::new(
                "odd",
                Reading::with_confidence(1.14, Duration::ZERO, 0.0),
            ))
            .unwrap();
        let default = f.measurement_variance(&tape("odd", 1.14)).unwrap();
        assert_eq!(zeroed, default);
    }

    /// Offer order must not decide anything: the batch update is order-free.
    #[test]
    fn offer_order_does_not_change_the_estimate() {
        let forward = fusion()
            .update(
                &set(&[tape("a", 1.140), tape("b", 1.1415), tape("c", 1.141)]),
                Some(1.141),
                BAND,
                secs(5),
            )
            .value;
        let reversed = fusion()
            .update(
                &set(&[tape("c", 1.141), tape("b", 1.1415), tape("a", 1.140)]),
                Some(1.141),
                BAND,
                secs(5),
            )
            .value;
        assert_eq!(forward, reversed);
    }

    #[test]
    fn the_default_config_validates() {
        assert_eq!(FusionConfig::default().validate(), Ok(()));
    }

    /// Each guarded constant is rejected, so the validator cannot read as
    /// having checked something it does not.
    #[test]
    fn degenerate_constants_are_rejected() {
        let bad = |f: fn(&mut FusionConfig)| {
            let mut cfg = FusionConfig::default();
            f(&mut cfg);
            assert!(cfg.validate().is_err());
        };
        bad(|c| c.drift_frac_per_sec = 0.0);
        bad(|c| c.tape_noise_frac = 0.0);
        bad(|c| c.reference_noise_frac = -1.0);
        bad(|c| c.reseed_floor_frac = f64::NAN);
        bad(|c| c.reseed_sigma = 0.0);
        // The upper half of the fraction predicate: a noise fraction above 1
        // means a sigma wider than the value itself. Covered because without a
        // case here half of `validate`'s condition could be deleted silently.
        bad(|c| c.tape_noise_frac = 2.0);
        bad(|c| c.drift_frac_per_sec = 1.5);
        // A zero interval is the absorb-every-tick behavior the throttle
        // exists to remove, so it is a degenerate setting and not an opt-out.
        bad(|c| c.reference_publish_interval = Duration::ZERO);
    }

    fn reference(source: &'static str, value: f64) -> Candidate {
        Candidate::reference(source, Reading::new(value, Duration::ZERO))
    }

    /// What the engine passes as `fast` for a **reference-only** leg.
    ///
    /// Not `None`, which is the trap: `fast` is documented as the tape-class
    /// median, but `FairValueEngine::compose` passes the leg's resolved
    /// reading, and the resolver falls back to the reference set when no tape
    /// answered (`consensus::a_leg_with_only_reference_sources_still_resolves`).
    /// Passing `None` here would trim every candidate away and test nothing —
    /// `admits` rejects everything when there is no consensus to measure by.
    fn resolved(value: f64) -> Option<f64> {
        Some(value)
    }

    /// The defect this mechanism exists to remove, stated as the arithmetic it
    /// broke: an unchanging reference fix must not sharpen the estimate tick
    /// after tick, because it is one observation and not thousands.
    ///
    /// The assertion is deliberately against the **single-observation
    /// variance** rather than against a remembered number. That is the quantity
    /// with a meaning — one daily fix cannot justify more confidence than one
    /// daily fix — so the test states the invariant instead of pinning
    /// whatever the filter happens to produce.
    #[test]
    fn a_reference_fix_is_absorbed_once_however_many_ticks_see_it() {
        let mut f = fusion();
        let c = set(&[reference("frankfurter", 1.14)]);

        // The seeding tick takes it in at its own variance.
        let seeded = f.update(&c, resolved(1.14), BAND, secs(5));
        assert_eq!(seeded.step, FusionStep::Seeded);
        assert_eq!(seeded.n, 1);
        let at_absorption = seeded.variance;

        // An hour of 5 s ticks offering the very same fix.
        let mut last = seeded;
        for _ in 0..720 {
            last = f.update(&c, resolved(1.14), BAND, secs(5));
        }

        // Nothing was fused on any of them, and the estimate is unmoved.
        assert_eq!(last.step, FusionStep::Carried);
        assert_eq!(last.n, 0);
        assert_eq!(last.value, seeded.value);

        // The variance GREW on drift rather than converging. Before this
        // mechanism it fell toward sqrt(q * dt * R), several-fold tighter than
        // one observation justifies; the direction of this inequality is the
        // whole fix.
        assert!(
            last.variance > at_absorption,
            "an unchanging fix must widen the estimate, not sharpen it: \
             {at_absorption} -> {}",
            last.variance
        );

        // And it grew by exactly the random walk over the elapsed hour, which
        // is the only term entitled to move it.
        let cfg = FusionConfig::default();
        let drift = cfg.drift_frac_per_sec * 1.14;
        let expected = at_absorption + drift * drift * 3_600.0;
        assert!((last.variance - expected).abs() < 1e-12 * expected);
    }

    /// The throttle is scoped to the reference class: a tape is fresh evidence
    /// on every poll and must keep being absorbed, or the fix would trade one
    /// calibration error for another.
    #[test]
    fn a_tape_is_still_absorbed_every_tick() {
        let mut f = fusion();
        let c = set(&[tape("oanda", 1.14)]);

        let seeded = f.update(&c, Some(1.14), BAND, secs(5));
        let mut last = seeded;
        for _ in 0..20 {
            last = f.update(&c, Some(1.14), BAND, secs(5));
        }

        assert_eq!(last.step, FusionStep::Fused);
        assert_eq!(last.n, 1);
        assert!(
            last.variance < seeded.variance,
            "repeated tape polls are independent observations and must sharpen"
        );
    }

    /// Once the configured interval has passed the fix counts again, and the
    /// variance snaps back to roughly one observation's worth — the reset half
    /// of the sawtooth the module header describes.
    #[test]
    fn the_fix_counts_again_once_its_publication_interval_has_passed() {
        let mut f = fusion();
        let c = set(&[reference("frankfurter", 1.14)]);

        let seeded = f.update(&c, resolved(1.14), BAND, secs(5));
        // Held off for just under a day...
        let held = f.update(&c, resolved(1.14), BAND, secs(86_000));
        assert_eq!(held.step, FusionStep::Carried);
        assert!(held.variance > seeded.variance);

        // ...and taken in again once the interval is met.
        let absorbed = f.update(&c, resolved(1.14), BAND, secs(400));
        assert_eq!(absorbed.step, FusionStep::Fused);
        assert_eq!(absorbed.n, 1);
        assert!(
            absorbed.variance < held.variance,
            "a genuinely new fix must sharpen the estimate again"
        );
    }

    /// A throttled source still appears in the attribution, at weight zero —
    /// the same treatment a trimmed source gets, and for the related reason
    /// that its reading is what the estimate is resting on.
    #[test]
    fn a_throttled_source_is_reported_rather_than_dropped() {
        let mut f = fusion();
        let c = set(&[reference("frankfurter", 1.14)]);

        f.update(&c, resolved(1.14), BAND, secs(5));
        let carried = f.update(&c, resolved(1.14), BAND, secs(5));

        let rows: Vec<_> = carried.contributions().collect();
        assert_eq!(rows.len(), 1, "the source answered and must be listed");
        assert_eq!(rows[0].source, "frankfurter");
        assert_eq!(rows[0].value, 1.14);
        assert_eq!(rows[0].weight, 0.0, "it contributed no precision");
    }

    /// A trimmed reading is not an absorbed one. The two exclusions are
    /// independent, so a source the trim rejected must stay eligible — losing
    /// that would let a single tick of disagreement silence a fix for a whole
    /// publication interval.
    #[test]
    fn a_trimmed_reference_is_not_recorded_as_absorbed() {
        let mut f = fusion();
        // Seed the filter off a tape so there is a fast consensus to trim
        // against, and offer a fix far outside the band.
        let far = set(&[tape("oanda", 1.14), reference("frankfurter", 1.60)]);
        let seeded = f.update(&far, Some(1.14), BAND, secs(5));
        assert_eq!(seeded.n, 1, "the fix was trimmed, the tape was not");

        // The fix now agrees, on the very next tick. It was never absorbed, so
        // the throttle must not be holding it off.
        let near = set(&[tape("oanda", 1.14), reference("frankfurter", 1.141)]);
        let fused = f.update(&near, Some(1.14), BAND, secs(5));
        assert_eq!(fused.n, 2, "the fix must be eligible, having never counted");
    }

    /// Absorption is tracked per source, so one source's throttle never
    /// silences another's — including when both are reference class.
    #[test]
    fn absorption_is_tracked_per_source() {
        let mut f = fusion();
        let first = set(&[reference("frankfurter", 1.14)]);
        f.update(&first, resolved(1.14), BAND, secs(5));

        // A second reference source appears, never yet absorbed.
        let both = set(&[reference("frankfurter", 1.14), reference("erapi", 1.141)]);
        let r = f.update(&both, resolved(1.14), BAND, secs(5));

        assert_eq!(r.n, 1, "only the newcomer counts");
        let erapi = r
            .contributions()
            .find(|c| c.source == "erapi")
            .expect("the newcomer is attributed");
        assert!(erapi.weight > 0.0, "and it carries the whole contribution");
        let frank = r
            .contributions()
            .find(|c| c.source == "frankfurter")
            .expect("the throttled source is still listed");
        assert_eq!(frank.weight, 0.0);
    }

    /// A leg with a live tape alongside the fix is unaffected: the tape keeps
    /// the filter fusing every tick, and the fix simply stops padding the
    /// precision. This is the case the roster actually runs, so it is the one
    /// that must not regress.
    #[test]
    fn a_tape_beside_a_throttled_fix_still_fuses_every_tick() {
        let mut f = fusion();
        let c = set(&[tape("oanda", 1.14), reference("frankfurter", 1.141)]);

        f.update(&c, Some(1.14), BAND, secs(5));
        let later = f.update(&c, Some(1.14), BAND, secs(5));

        assert_eq!(later.step, FusionStep::Fused);
        assert_eq!(later.n, 1, "the tape alone, the fix already absorbed");
        assert_eq!(later.contributions().count(), 2, "both still reported");
    }
}
