//! Multi-source consensus for one leg (§1 leg resolution).
//!
//! A leg used to resolve through a fixed priority ladder: the first tier that
//! answered won outright, so **a single bad source became the answer** with
//! nothing to contradict it. The ladder is a fallback mechanism, and it was
//! being asked to serve as a truth mechanism.
//!
//! The failure is not hypothetical and not confined to one market. Only one
//! roster market reaches a CEX primary at all; for the rest the aggregator
//! fallback *is* the basis leg, which means an unverified single source sits
//! under most of the book. A thin market is simply where it shows first — an
//! aggregate built from a few hundred dollars of daily volume printed roughly
//! half its peg, and because there was no second source, that number sailed
//! into the engine and lit the peg alarm permanently.
//!
//! So a leg now collects **every healthy source** rather than the best one, and
//! resolves them together:
//!
//! - **three or more** — take the **median**, which is robust to one bad
//!   source in a way no average is;
//! - **two** — usable only if they agree within the dispersion band; two
//!   readings that disagree cannot adjudicate between themselves, so the leg
//!   degrades rather than guessing;
//! - **one** — an explicit single-source state. Believed outright only for a
//!   source designated trustworthy; otherwise it still carries the mid (most of
//!   the roster has nothing else) but the composition is marked
//!   [`crate::Health::Unverified`], which is precisely the existing "quoting on
//!   a reference nothing corroborates" signal.
//!
//! The **dispersion gate** rides alongside: when the healthy set's spread
//! exceeds the band, the leg is flagged and the furthest source named. With
//! three or more the median still stands (that is what robustness buys); with
//! two there is nothing to stand on.
//!
//! The tier order does not disappear — it survives as *ordering metadata*,
//! deciding who is preferred when the set is thin, rather than deciding the
//! answer.

use std::time::Duration;

use crate::reading::Reading;

/// How many sources one leg may carry. A fixed array keeps [`crate::Legs`]
/// `Copy` and keeps this crate allocation-free.
///
/// Sized with headroom above the longest roster leg (four: two venues and two
/// aggregators) rather than exactly to it. That margin is load-bearing:
/// candidates are placed in the order offered, before anything knows which are
/// healthy, so a leg filled to the cap could seat a **stale** candidate ahead of
/// a live source and silently drop the live one. Keeping the cap clear of the
/// real ladders means that cannot arise, and adding a source does not quietly
/// evict another. A leg that does overflow drops its least-preferred
/// candidates — the only thing offer order is still entitled to decide.
pub const MAX_CANDIDATES: usize = 6;

/// The longest ladder any roster leg offers today: two venues and two
/// aggregators on the basis leg.
const LONGEST_REAL_LEG: usize = 4;

// Enforced at compile time rather than in a test, because the margin is the
// whole reason the cap is not simply `LONGEST_REAL_LEG`: without it a stale
// candidate placed before a live one could evict it.
const _: () = assert!(MAX_CANDIDATES > LONGEST_REAL_LEG);

/// One source's reading for a leg, tagged so a disagreement can name who
/// diverged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Stable identifier for the source, for the operator view. The crate never
    /// interprets it — venue identity belongs to the consumer.
    pub source: &'static str,
    /// What the source published.
    pub reading: Reading,
    /// Whether this source may be believed with nothing corroborating it.
    ///
    /// A designation, not a quality score: it says the operator has accepted
    /// this source as sufficient on its own (a first-party FX oracle, say),
    /// which is a different claim from it being the most accurate. Everything
    /// else alone is reported as uncorroborated rather than silently trusted.
    pub trusted: bool,
}

impl Candidate {
    /// A candidate that needs corroboration.
    pub fn new(source: &'static str, reading: Reading) -> Self {
        Self {
            source,
            reading,
            trusted: false,
        }
    }

    /// A candidate believable on its own — see [`Candidate::trusted`].
    pub fn trusted(source: &'static str, reading: Reading) -> Self {
        Self {
            source,
            reading,
            trusted: true,
        }
    }
}

/// Every source offered for one leg this tick, in preference order.
///
/// Order carries no weight in the resolution below — it is retained only so a
/// thin set falls back predictably, which is all the old tier ladder is still
/// entitled to decide.
#[derive(Clone, Copy, Debug, Default)]
pub struct Candidates {
    slots: [Option<Candidate>; MAX_CANDIDATES],
}

impl Candidates {
    /// An empty set — the leg had no source answer at all.
    pub fn none() -> Self {
        Self::default()
    }

    /// A set holding one candidate.
    pub fn one(candidate: Candidate) -> Self {
        Self::none().with(Some(candidate))
    }

    /// Add a candidate, if the source answered. Silently ignores a `None` (a
    /// source that did not answer is not a candidate) and any push beyond
    /// [`MAX_CANDIDATES`], so a caller can offer its whole ladder unconditionally.
    #[must_use]
    pub fn with(mut self, candidate: Option<Candidate>) -> Self {
        if let Some(c) = candidate {
            if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
                *slot = Some(c);
            }
        }
        self
    }

    /// Add a source's reading when it published one.
    #[must_use]
    pub fn push(self, source: &'static str, reading: Option<Reading>) -> Self {
        self.with(reading.map(|r| Candidate::new(source, r)))
    }

    /// Add a reading from a source believable on its own.
    #[must_use]
    pub fn push_trusted(self, source: &'static str, reading: Option<Reading>) -> Self {
        self.with(reading.map(|r| Candidate::trusted(source, r)))
    }

    /// Every candidate offered, healthy or not.
    pub fn iter(&self) -> impl Iterator<Item = &Candidate> {
        self.slots.iter().flatten()
    }

    /// Whether any source answered at all this tick — regardless of health.
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Whether some source answered promptly but with an unusable value. Lets a
    /// caller tell a live feed publishing garbage from a dead one.
    pub fn any_invalid(&self, stale: Duration) -> bool {
        self.iter()
            .any(|c| c.reading.young(stale) && !c.reading.valid())
    }

    /// Resolve the healthy candidates into one reading for the leg.
    ///
    /// `dispersion_frac` is the fraction of the consensus value the healthy
    /// set's spread may span before the leg is flagged as dispersed.
    pub fn resolve(&self, stale: Duration, dispersion_frac: f64) -> Consensus {
        let mut healthy = [None; MAX_CANDIDATES];
        let mut n = 0;
        for c in self.iter().filter(|c| c.reading.fresh(stale)) {
            healthy[n] = Some(*c);
            n += 1;
        }

        let Some(first) = healthy[0] else {
            return Consensus {
                reading: None,
                state: ConsensusState::Absent,
                outlier: None,
                n: 0,
            };
        };

        if n == 1 {
            let state = if first.trusted {
                ConsensusState::SingleTrusted
            } else {
                ConsensusState::SingleUnverified
            };
            return Consensus {
                reading: Some(first.reading),
                state,
                outlier: None,
                n: 1,
            };
        }

        let mut values = [0.0f64; MAX_CANDIDATES];
        for (i, c) in healthy.iter().flatten().enumerate() {
            values[i] = c.reading.value;
        }
        let values = &mut values[..n];
        values.sort_by(|a, b| a.partial_cmp(b).expect("healthy readings are finite"));

        // The median is the reference for both the dispersion test and the
        // outlier attribution, because it is the one summary a single bad
        // source cannot move. Measuring either against a designated source
        // instead would make the majority look like the outliers whenever that
        // source was the broken one — which is the case the whole filter exists
        // to catch.
        let consensus = median(values);
        let spread = values[n - 1] - values[0];
        // A non-positive consensus cannot happen — every healthy reading is
        // positive — but dividing by it would be the kind of thing that only
        // shows up in production, so the guard stays.
        let dispersed = consensus > 0.0 && spread / consensus > dispersion_frac;
        let outlier = dispersed.then(|| furthest_from(&healthy[..n], consensus));

        // A source designated believable on its own **anchors** its leg: the
        // others corroborate it and can flag a disagreement, but they do not
        // blend into it. Averaging a live first-party oracle with a daily
        // reference rate would only degrade the anchor the leg exists to
        // provide, and the designation is precisely the standing statement that
        // this source does not need the others' help.
        //
        // The one thing that overrides it is being contradicted: if the trusted
        // source is itself the outlier, the majority stands and the median wins.
        // Otherwise a designation would become a way for one bad feed to beat
        // every check on it.
        let anchor = lone_trusted(&healthy[..n]).filter(|c| outlier != Some(c.source));

        let reading = match anchor {
            Some(c) => Some(representative(&[Some(c)], c.reading.value)),
            // With three or more the median is exactly what survives one bad
            // source, so a dispersed set still yields a usable value — flagged,
            // not discarded. With two there is no majority to appeal to and no
            // designation to break the tie, so the leg has nothing to offer.
            None if dispersed && n == 2 => None,
            None => Some(representative(&healthy[..n], consensus)),
        };

        Consensus {
            reading,
            state: if dispersed {
                ConsensusState::Dispersed
            } else if n >= 3 {
                ConsensusState::Corroborated
            } else {
                ConsensusState::Agreed
            },
            outlier,
            n,
        }
    }
}

/// How well corroborated a leg's value is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ConsensusState {
    /// No healthy source — the default, since a leg nobody reported on has
    /// nothing behind it.
    #[default]
    Absent,
    /// Three or more healthy sources within the dispersion band — the median.
    Corroborated,
    /// Exactly two healthy sources, agreeing within the band.
    Agreed,
    /// One healthy source, designated believable on its own.
    SingleTrusted,
    /// One healthy source, with nothing corroborating it. The value is still
    /// used — most of the roster has no second source — but the composition
    /// reports [`crate::Health::Unverified`] so the operator is not told a
    /// single unchecked feed is a corroborated price.
    SingleUnverified,
    /// The healthy sources disagree beyond the dispersion band.
    Dispersed,
}

impl ConsensusState {
    /// Whether the leg rests on a single uncorroborated source.
    pub fn is_uncorroborated(self) -> bool {
        matches!(self, Self::SingleUnverified)
    }
}

impl Consensus {
    /// The resolution of a leg with no healthy source.
    pub fn absent() -> Self {
        Self {
            reading: None,
            state: ConsensusState::Absent,
            outlier: None,
            n: 0,
        }
    }
}

/// The resolution of one leg's candidate set.
#[derive(Clone, Copy, Debug)]
pub struct Consensus {
    /// The agreed reading, or `None` when the leg has nothing usable.
    ///
    /// Its `value` is the consensus; its `age` is the **oldest** contributing
    /// age and its `confidence` the **widest** contributing half-width, so
    /// every downstream freshness and uncertainty test reads the most
    /// conservative view of the set rather than the flattering one.
    pub reading: Option<Reading>,
    /// How well corroborated the value is.
    pub state: ConsensusState,
    /// The source furthest from the consensus, when the set is dispersed.
    pub outlier: Option<&'static str>,
    /// How many healthy candidates contributed.
    pub n: usize,
}

/// The median of a sorted slice; the mean of the two middle values when even.
fn median(sorted: &[f64]) -> f64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    }
}

/// The single trusted candidate in a set, when there is exactly one. Two
/// trusted sources disagreeing is precisely the case no designation can settle.
fn lone_trusted(healthy: &[Option<Candidate>]) -> Option<Candidate> {
    let mut found = None;
    for c in healthy.iter().flatten().filter(|c| c.trusted) {
        if found.is_some() {
            return None;
        }
        found = Some(*c);
    }
    found
}

/// The source whose reading sits furthest from `consensus`.
///
/// Ties break toward the **untrusted** source. A pair straddling its own
/// midpoint is exactly tied, and naming the designated source there would be
/// both unhelpful and wrong: the standing judgement is that it is the one to
/// believe, so the other is the one to look at.
fn furthest_from(healthy: &[Option<Candidate>], consensus: f64) -> &'static str {
    healthy
        .iter()
        .flatten()
        .max_by(|a, b| {
            let (da, db) = (
                (a.reading.value - consensus).abs(),
                (b.reading.value - consensus).abs(),
            );
            da.partial_cmp(&db)
                .expect("healthy readings are finite")
                .then(b.trusted.cmp(&a.trusted))
        })
        .map(|c| c.source)
        .unwrap_or("unknown")
}

/// A reading carrying the consensus value, the oldest contributing age, and the
/// widest contributing confidence.
fn representative(healthy: &[Option<Candidate>], consensus: f64) -> Reading {
    let mut age = Duration::ZERO;
    let mut confidence: Option<f64> = None;
    for c in healthy.iter().flatten() {
        age = age.max(c.reading.age);
        if let Some(conf) = c.reading.confidence {
            confidence = Some(confidence.map_or(conf, |w: f64| w.max(conf)));
        }
    }
    Reading {
        value: consensus,
        age,
        confidence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn r(v: f64) -> Reading {
        Reading::new(v, secs(1))
    }

    const STALE: Duration = Duration::from_secs(300);
    /// A 2% dispersion band, tight enough that the cases below are unambiguous.
    const BAND: f64 = 0.02;

    #[test]
    fn an_empty_set_resolves_to_absent() {
        let c = Candidates::none().resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Absent);
        assert!(c.reading.is_none());
        assert_eq!(c.n, 0);
    }

    #[test]
    fn a_lone_untrusted_source_is_used_but_marked() {
        // Most of the roster is here: one aggregator and nothing else. It must
        // still carry the mid, or those markets stop quoting entirely — but it
        // must not be reported as a corroborated price.
        let c = Candidates::none()
            .push("coingecko", Some(r(1.14)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::SingleUnverified);
        assert!(c.state.is_uncorroborated());
        assert_eq!(c.reading.unwrap().value, 1.14);
    }

    #[test]
    fn a_lone_trusted_source_is_believed() {
        let c = Candidates::none()
            .push_trusted("pyth", Some(r(1.14)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::SingleTrusted);
        assert!(!c.state.is_uncorroborated());
    }

    #[test]
    fn two_agreeing_sources_average() {
        let c = Candidates::none()
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.142)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Agreed);
        assert!((c.reading.unwrap().value - 1.141).abs() < 1e-12);
        assert_eq!(c.n, 2);
    }

    #[test]
    fn two_disagreeing_untrusted_sources_cannot_adjudicate() {
        // Neither can be preferred, so the leg offers nothing rather than
        // picking the one that happens to be listed first.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.14)))
            .push("coingecko", Some(r(0.60)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert!(c.reading.is_none());
        assert!(c.outlier.is_some());
    }

    #[test]
    fn a_trusted_source_anchors_its_leg_rather_than_averaging_into_it() {
        // Corroboration is a check, not a blend. Averaging a live first-party
        // oracle with a daily reference would drag the anchor toward the slower
        // source every tick — degrading the very thing the leg exists to supply
        // — while telling the operator nothing extra. The pair still counts as
        // agreed, which is what the second source is for.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.1500)))
            .push("frankfurter", Some(r(1.1400)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Agreed);
        assert_eq!(c.reading.unwrap().value, 1.1500, "not the 1.145 midpoint");
        assert_eq!(c.n, 2);
    }

    #[test]
    fn a_trusted_source_keeps_its_own_age_and_confidence() {
        // Following from the rule above: if the anchor is not blended, it must
        // not inherit the slower source's age or the wider one's half-width
        // either, or the fresh-but-uncertain regime would fire on a reading
        // that is neither.
        let c = Candidates::none()
            .push_trusted(
                "pyth-hermes",
                Some(Reading::with_confidence(1.1500, secs(1), 0.0001)),
            )
            .push("frankfurter", Some(Reading::new(1.1400, secs(200))))
            .resolve(STALE, BAND);
        let reading = c.reading.unwrap();
        assert_eq!(reading.age, secs(1));
        assert_eq!(reading.confidence, Some(0.0001));
    }

    #[test]
    fn a_lone_trusted_source_breaks_a_disagreeing_pair() {
        // The live oracle against the daily reference: they can legitimately
        // drift apart, and the trust designation already says which one to take
        // alone. The leg is still flagged as dispersed — the disagreement is
        // real and worth surfacing — but it does not go dark.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.14)))
            .push("frankfurter", Some(r(1.05)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert_eq!(c.reading.unwrap().value, 1.14, "the trusted source stands");
        assert_eq!(c.outlier, Some("frankfurter"));
    }

    #[test]
    fn two_disagreeing_trusted_sources_still_cannot_adjudicate() {
        // A designation only settles a tie when it picks out one source. Two
        // sources both believable alone, disagreeing, is exactly the case
        // nothing can settle.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.14)))
            .push_trusted("oanda", Some(r(1.05)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert!(c.reading.is_none());
    }

    #[test]
    fn a_broken_trusted_source_does_not_override_a_majority() {
        // Trust breaks a tie; it does not beat a median. With three sources the
        // robust estimate stands, even if the outlier is the trusted one —
        // otherwise a designation would become a way for one bad feed to win,
        // which is the failure this whole filter exists to remove.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(0.60)))
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.141)))
            .resolve(STALE, BAND);
        assert_eq!(c.reading.unwrap().value, 1.140, "the median, not the pin");
        assert_eq!(c.outlier, Some("pyth-hermes"));
    }

    #[test]
    fn three_sources_take_the_median_and_ignore_one_bad_one() {
        // The whole point: the bad source moves nothing. An average of these
        // three would land near 0.95 and quote the book badly.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.14)))
            .push("kraken", Some(r(1.141)))
            .push("coingecko", Some(r(0.57))) // the thin-aggregate shape
            .resolve(STALE, BAND);
        assert_eq!(c.reading.unwrap().value, 1.14, "the median, not the mean");
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert_eq!(c.outlier, Some("coingecko"));
    }

    #[test]
    fn three_agreeing_sources_are_corroborated() {
        let c = Candidates::none()
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.141)))
            .push("coingecko", Some(r(1.142)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Corroborated);
        assert_eq!(c.reading.unwrap().value, 1.141);
        assert_eq!(c.outlier, None);
    }

    #[test]
    fn four_sources_take_the_middle_pair() {
        let c = Candidates::none()
            .push("a", Some(r(1.140)))
            .push("b", Some(r(1.141)))
            .push("c", Some(r(1.142)))
            .push("d", Some(r(1.143)))
            .resolve(STALE, BAND);
        assert_eq!(c.n, 4);
        assert!((c.reading.unwrap().value - 1.1415).abs() < 1e-12);
    }

    #[test]
    fn unhealthy_candidates_do_not_count_toward_the_set() {
        // A stale reading and a garbage one are not opinions.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.14)))
            .push("kraken", Some(Reading::new(1.14, secs(600))))
            .push("coingecko", Some(Reading::new(f64::NAN, secs(1))))
            .resolve(STALE, BAND);
        assert_eq!(c.n, 1, "only the fresh, valid one counts");
        assert_eq!(c.state, ConsensusState::SingleUnverified);
    }

    #[test]
    fn a_source_that_did_not_answer_is_not_a_candidate() {
        let c = Candidates::none()
            .push("coinbase", None)
            .push("kraken", Some(r(1.14)));
        assert_eq!(c.iter().count(), 1);
        assert!(!c.is_empty());
        assert!(Candidates::none().is_empty());
    }

    #[test]
    fn the_set_reports_the_most_conservative_age_and_confidence() {
        // The consensus must not look fresher or tighter than its worst
        // contributor, or the freshness and uncertainty gates read the
        // flattering view of a set they are meant to police.
        let c = Candidates::none()
            .with(Some(Candidate::new(
                "a",
                Reading::with_confidence(1.140, secs(1), 0.001),
            )))
            .with(Some(Candidate::new(
                "b",
                Reading::with_confidence(1.142, secs(30), 0.02),
            )))
            .resolve(STALE, BAND);
        let reading = c.reading.unwrap();
        assert_eq!(reading.age, secs(30), "the oldest contributing age");
        assert_eq!(reading.confidence, Some(0.02), "the widest half-width");
    }

    #[test]
    fn overflowing_the_ladder_keeps_the_preferred_sources() {
        // Order is only entitled to decide who survives an over-full set, but
        // it must decide it predictably.
        let mut c = Candidates::none();
        for source in ["a", "b", "c", "d", "e", "f"] {
            c = c.push(source, Some(r(1.0)));
        }
        c = c.push("overflow", Some(r(9.0)));
        assert_eq!(c.iter().count(), MAX_CANDIDATES);
        assert!(c.iter().all(|x| x.source != "overflow"));
    }

    #[test]
    fn a_full_real_leg_still_has_room_to_spare() {
        // The margin that keeps a stale candidate from ever displacing a live
        // one is asserted at compile time above; this checks the thing that
        // margin is *for* — offering the longest real ladder leaves every
        // source in place, with room for one more.
        let mut c = Candidates::none();
        for source in ["coinbase", "kraken", "coingecko", "coinmarketcap"] {
            c = c.push(source, Some(r(1.0)));
        }
        assert_eq!(c.iter().count(), LONGEST_REAL_LEG);
        assert_eq!(c.push("a-fifth", Some(r(1.0))).iter().count(), 5);
    }

    #[test]
    fn a_stale_candidate_never_masks_a_live_source() {
        // The invariant the old tier walk had to enforce by hand, and which the
        // caches make load-bearing: they never evict, so a source that dies once
        // would otherwise sit in front of its fallbacks for the life of the
        // process. Here a dead source is simply not a candidate, whatever
        // position it was offered in.
        let c = Candidates::none()
            .push("dead-primary", Some(Reading::new(1.14, secs(9_999))))
            .push("live-fallback", Some(r(0.99)))
            .resolve(STALE, BAND);
        assert_eq!(c.n, 1);
        assert_eq!(c.reading.unwrap().value, 0.99);
    }

    #[test]
    fn invalid_is_distinguishable_from_absent() {
        let live_garbage = Candidates::none().push("a", Some(Reading::new(0.0, secs(1))));
        assert!(live_garbage.any_invalid(STALE));

        let dead = Candidates::none().push("a", Some(Reading::new(1.14, secs(600))));
        assert!(!dead.any_invalid(STALE), "stale is not invalid");
        assert!(!Candidates::none().any_invalid(STALE));
    }
}
