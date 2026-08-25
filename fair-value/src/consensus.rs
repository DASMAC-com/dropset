//! Multi-source consensus for one leg (§1 leg resolution).
//!
//! A leg used to resolve through a fixed priority ladder: the first tier that
//! answered won outright, so **a single bad source became the answer** with
//! nothing to contradict it. The ladder is a fallback mechanism, and it was
//! being asked to serve as a truth mechanism.
//!
//! The failure is not hypothetical and not confined to one market. Only one
//! roster market reaches a CEX primary at all; for five of the rest an
//! aggregator index *is* the basis leg, so an unverified single source sits
//! under most of the book. (The remaining market has no basis source whatever —
//! its basis is pinned, so there is nothing here to resolve for it.) A thin
//! market is simply where it shows first — an aggregate built from a few hundred
//! dollars of daily volume printed roughly half its peg, and because there was
//! no second source, that number sailed into the engine and lit the peg alarm
//! permanently.
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

/// How a source publishes — the property that decides whether its reading may
/// be pooled with another's.
///
/// Not a quality ranking, and not interchangeable with [`Candidate::trusted`]:
/// this says how *often* a source speaks and what its number is a statement
/// about, while `trusted` says whether it may be believed alone. A daily
/// central-bank fix scores high on the second and low on the first.
///
/// The distinction exists because pooling sources with different publication
/// conventions is a standing hazard: a daily fix six hours old and a minute tape
/// are both "the EUR/USD rate", but only one of them is a claim about now.
/// Dropping both into one median lets the stale one drag the fast signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SourceClass {
    /// A tape: publishes at minute-or-better cadence and tracks the live market.
    /// The default, because every source predating this distinction is one.
    #[default]
    Tape,
    /// A reference fix: published on a slow schedule (daily, typically), and
    /// authoritative for the moment it names rather than for now. Central-bank
    /// reference rates and the open exchange-rate services are this class.
    ///
    /// Kept **out of the fast consensus median** that guards dislocations, and
    /// **in the fusion estimator**, where it enters as a timestamped
    /// wide-variance measurement (see [`crate::Fusion`]). That split is what
    /// keeps the fix's information content without letting it drag the fast
    /// signal.
    Reference,
}

impl SourceClass {
    /// Whether a source of this class may contribute to the fast consensus
    /// median.
    pub fn is_fast(self) -> bool {
        matches!(self, Self::Tape)
    }
}

/// One source's reading for a leg, tagged so a disagreement can name who
/// diverged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// Stable identifier for the source, for the operator view. The crate never
    /// interprets it — venue identity belongs to the consumer.
    pub source: &'static str,
    /// What the source published.
    pub reading: Reading,
    /// How this source publishes — see [`SourceClass`].
    pub class: SourceClass,
    /// Whether this source may be believed with nothing corroborating it.
    ///
    /// A designation, not a quality score: it says the operator has accepted
    /// this source as sufficient on its own (a first-party FX oracle, say),
    /// which is a different claim from it being the most accurate. Everything
    /// else alone is reported as uncorroborated rather than silently trusted.
    pub trusted: bool,
}

impl Candidate {
    /// A tape candidate that needs corroboration.
    pub fn new(source: &'static str, reading: Reading) -> Self {
        Self {
            source,
            reading,
            trusted: false,
            class: SourceClass::Tape,
        }
    }

    /// A tape candidate believable on its own — see [`Candidate::trusted`].
    pub fn trusted(source: &'static str, reading: Reading) -> Self {
        Self {
            source,
            reading,
            trusted: true,
            class: SourceClass::Tape,
        }
    }

    /// A [`SourceClass::Reference`] candidate — a slow reference fix, fused but
    /// kept out of the fast median.
    pub fn reference(source: &'static str, reading: Reading) -> Self {
        Self {
            source,
            reading,
            trusted: false,
            class: SourceClass::Reference,
        }
    }

    /// This candidate, designated believable with nothing corroborating it.
    #[must_use]
    pub fn believed_alone(mut self) -> Self {
        self.trusted = true;
        self
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

    /// Add a candidate, if the source answered. Silently ignores a `None` (a
    /// source that did not answer is not a candidate) and any push beyond
    /// [`MAX_CANDIDATES`], so a caller can offer its whole ladder unconditionally.
    ///
    /// Private: [`Candidates::push`] and [`Candidates::push_trusted`] are the
    /// surface callers want, and they carry the source name a bare `Candidate`
    /// would let a caller forget.
    #[must_use]
    fn with(mut self, candidate: Option<Candidate>) -> Self {
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

    /// Add a reading from a slow [`SourceClass::Reference`] source — a daily
    /// central-bank fix or an open exchange-rate service.
    ///
    /// The reading is fused but kept out of the fast median, so offering one
    /// alongside a live tape strengthens the estimate without slowing the
    /// dislocation guard. On a leg where it is the *only* source it still
    /// resolves the leg — see [`Candidates::resolve`].
    #[must_use]
    pub fn push_reference(self, source: &'static str, reading: Option<Reading>) -> Self {
        self.with(reading.map(|r| Candidate::reference(source, r)))
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

    /// Every healthy candidate's value, in offer order.
    ///
    /// For a caller that must reason about the individual readings rather than
    /// the consensus — a guard whose job is "did **anything** report a problem?"
    /// rather than "what is this leg worth?". Those are different questions, and
    /// answering the first from the consensus is how a guard gets silenced by
    /// the very disagreement it exists to catch.
    pub fn healthy_values(&self, stale: Duration) -> impl Iterator<Item = f64> + '_ {
        self.iter()
            .filter(move |c| c.reading.fresh(stale))
            .map(|c| c.reading.value)
    }

    /// Resolve the healthy candidates into one **fast** reading for the leg.
    ///
    /// `dispersion_frac` is the fraction of the consensus value the healthy
    /// set's spread may span before the leg is flagged as dispersed.
    ///
    /// # Which candidates decide the value
    ///
    /// The resolution below runs over the **tape-class** healthy candidates
    /// only, so a slow reference fix never drags the fast signal the dislocation
    /// guard depends on. Reference-class candidates are still returned in
    /// [`Consensus::healthy`], which is what the fusion estimator consumes — the
    /// split is between the two *uses*, not between which sources are collected.
    ///
    /// **A leg with no tape source at all falls back to its reference
    /// candidates**, and that fallback is load-bearing rather than a
    /// convenience. Several markets are anchored on a daily fix and nothing
    /// else; excluding reference sources unconditionally would resolve those
    /// legs to `Absent` and dark them outright. The hazard the exclusion exists
    /// to prevent is *pooling* two publication conventions in one median, and a
    /// leg with only one convention present is not pooling anything.
    ///
    /// Every count below — the median, the pair band, the single-source states,
    /// the dispersion gate — is therefore a statement about the fast set. That
    /// is the honest reading: `n` is how many sources corroborate the fast
    /// signal, and a reference fix does not corroborate it.
    pub fn resolve(&self, stale: Duration, dispersion_frac: f64) -> Consensus {
        // Both fills zip against the destination array, so a set larger than
        // `MAX_CANDIDATES` drops its trailing candidates rather than panicking
        // on an index — the documented overflow behavior, and the only thing
        // offer order is still entitled to decide.
        let mut all = [None; MAX_CANDIDATES];
        for (slot, c) in all
            .iter_mut()
            .zip(self.iter().filter(|c| c.reading.fresh(stale)))
        {
            *slot = Some(*c);
        }

        // The fast set: tape class where any answered, else the whole healthy
        // set — see the note above on why the fallback is not optional.
        let any_fast = all.iter().flatten().any(|c| c.class.is_fast());
        let mut healthy = [None; MAX_CANDIDATES];
        for (slot, c) in healthy.iter_mut().zip(
            all.iter()
                .flatten()
                .filter(|c| !any_fast || c.class.is_fast()),
        ) {
            *slot = Some(*c);
        }
        let n = healthy.iter().flatten().count();

        let Some(first) = healthy[0] else {
            return Consensus {
                reading: None,
                state: ConsensusState::Absent,
                contributors: Contributors::none(),
                outlier: None,
                n: 0,
                healthy: all,
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
                // The value *is* this source's reading, so attributing it
                // wholly to that source is truthful whether or not the source
                // is designated. Corroboration is `state`'s business, not the
                // contributor set's: this says where the number came from, not
                // how much it should be believed.
                contributors: Contributors::one(first),
                outlier: None,
                n: 1,
                healthy: all,
            };
        }

        // Rank a **copy** by value. The median needs an ordering, but `healthy`
        // has to keep offer order: `furthest_from` breaks an exact distance tie
        // on iteration order, so reordering it in place would silently change
        // which source a dispersed set names.
        let mut ranked = healthy;
        let ranked = &mut ranked[..n];
        ranked.sort_by(|a, b| {
            // The healthy set is compacted into slots `0..n` above, so no
            // interior `None` reaches this comparator; the `INFINITY` arm only
            // satisfies the type.
            let value = |c: &Option<Candidate>| c.map_or(f64::INFINITY, |c| c.reading.value);
            value(a)
                .partial_cmp(&value(b))
                .expect("healthy readings are finite")
        });

        let mut values = [0.0f64; MAX_CANDIDATES];
        for (i, c) in ranked.iter().flatten().enumerate() {
            values[i] = c.reading.value;
        }
        let values = &values[..n];

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
        // A non-finite consensus cannot be summarized at all: with an even count
        // the median averages the two middle values, so a pair near `f64::MAX`
        // overflows to infinity. Testing dispersion first would then divide the
        // spread by infinity, get zero, and report the widest possible set as
        // perfect agreement — so this is handled before the band, not folded
        // into it. Such a set is maximally dispersed and has nothing to offer;
        // the outlier is measured from the smallest value rather than the
        // consensus, since distances to infinity are meaningless.
        if !consensus.is_finite() {
            return Consensus {
                reading: None,
                state: ConsensusState::Dispersed,
                contributors: Contributors::none(),
                outlier: Some(furthest_from(&healthy[..n], values[0])),
                n,
                healthy: all,
            };
        }

        let dispersed = consensus > 0.0 && spread / consensus > dispersion_frac;

        let trusted = lone_trusted(&healthy[..n]);

        // Naming the outlier: normally the source furthest from the median. But
        // with exactly two sources the median IS their midpoint, so both sit the
        // same distance from it and the comparison decides nothing — it is
        // settled by whether `a + b` happened to be exactly representable.
        // Measured, that names the designated source about a quarter of the
        // time, which would drop the anchor and dark the leg in precisely the
        // case the designation exists to rescue. So for a pair, the designation
        // names the outlier directly rather than being inferred from arithmetic
        // that carries no signal.
        let outlier = dispersed.then(|| match (n, trusted) {
            (2, Some(t)) => partner_of(&healthy[..n], t.source),
            _ => furthest_from(&healthy[..n], consensus),
        });

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
        // every check on it. With only two sources there is no majority to do
        // the contradicting, which is why the pair case is decided above.
        let anchor = trusted.filter(|c| outlier != Some(c.source));

        // The reading and its attribution are decided by **one** match, so the
        // two cannot drift apart: every arm that yields a value names what
        // composed it, and the arm that yields nothing names nothing. Splitting
        // this into two matches is how a contributor set ends up describing a
        // value the resolver did not actually return.
        let (reading, contributors) = match anchor {
            // An anchor does not blend, so it is the whole answer — the one
            // case where the old ladder's single winner is still honest.
            Some(c) => (
                Some(representative(&[Some(c)], c.reading.value)),
                Contributors::one(c),
            ),
            // With three or more the median is exactly what survives one bad
            // source, so a dispersed set still yields a usable value — flagged,
            // not discarded. With two there is no majority to appeal to and no
            // designation to break the tie, so the leg has nothing to offer.
            None if dispersed && n == 2 => (None, Contributors::none()),
            None => (
                Some(representative(&healthy[..n], consensus)),
                middle_of(ranked),
            ),
        };

        Consensus {
            reading,
            contributors,
            state: if dispersed {
                ConsensusState::Dispersed
            } else if n >= 3 {
                ConsensusState::Corroborated
            } else {
                ConsensusState::Agreed
            },
            outlier,
            n,
            healthy: all,
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
    ///
    /// Deliberately **not** true of [`ConsensusState::Dispersed`], even though a
    /// disagreeing set is also uncorroborated in the plain sense. The two want
    /// opposite treatment: a lone source is a permanent condition and must not
    /// tighten the kill switches forever, while a disagreement is a fault and
    /// should — so dispersion travels on its own path (see
    /// `Degrade::LegDispersed`) rather than through this one.
    pub fn is_uncorroborated(self) -> bool {
        matches!(self, Self::SingleUnverified)
    }

    /// Whether the leg's healthy sources disagree beyond the dispersion band.
    pub fn is_dispersed(self) -> bool {
        matches!(self, Self::Dispersed)
    }
}

/// One source's share of a resolved leg's value.
///
/// The old priority ladder had a single well-defined "tier that answered", and
/// consumers were built on it. Under consensus that concept does not survive in
/// general: the value usually belongs to the *set*, so naming one contributor
/// would resurrect ladder semantics as a lie dressed as data.
///
/// The weights here are **exact rather than heuristic**, because every
/// resolution is a linear combination of contributor values — see
/// [`Consensus::contributors`] for the case table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contributor {
    /// The contributing source's tag, exactly as offered on its [`Candidate`].
    ///
    /// This is the **bare venue** vocabulary, which is not always the feed
    /// adapter's own `Source::name()` — and the difference is load-bearing for
    /// anything that joins the two, because a mismatched join fails silently
    /// rather than loudly. Most sources name themselves identically either way;
    /// a venue whose endpoint is per *product* names itself per product
    /// (`coinbase:EURC-USDC`) while this tag stays the bare `coinbase`.
    ///
    /// Widening this to the adapter's name is deliberately **not** the fix: a
    /// per-product name is `format!`-built, so carrying one would cost a heap
    /// allocation per contributor per tick and take `Copy` off every leg type
    /// on the quoting hot path. A consumer joining to a per-feed health table
    /// matches on the `:` prefix rather than on equality.
    pub source: &'static str,
    /// This source's share of the resolved value, in `0.0..=1.0`. The weights
    /// of a set sum to 1 whenever the leg resolved to anything at all.
    pub weight: f64,
    /// How old *this* source's reading was.
    ///
    /// **Diagnostic only.** The leg's age is [`Consensus::reading`]`.age`, and
    /// no freshness or staleness test may read this field in its place — that
    /// would be reading a flattering view of the very set it exists to police.
    ///
    /// Be exact about how the two relate, because it differs **by arm** and the
    /// intuitive reading is wrong in one of them:
    ///
    /// - Resolved to a **median**, the leg's age is the oldest across *every
    ///   healthy candidate*, including the outer members that carry no weight.
    ///   They were still part of the set that got judged, so the leg can be
    ///   older than every contributor named here — and excluding them would let
    ///   a stale outer source vanish from the one number that polices staleness.
    /// - **Anchored** by a designated source, the others neither enter the value
    ///   nor its age: the leg carries the anchor's own age alone. A stale
    ///   corroborator does *not* raise it, which is the same judgement as not
    ///   letting one drag the value — the designation says this source stands
    ///   without their help.
    /// - A **lone** source is both at once, so the two readings coincide.
    ///
    /// Either way these ages account for a contributor's own freshness, not
    /// necessarily the leg's, and no freshness or staleness test may read one in
    /// place of [`Consensus::reading`]`.age`.
    pub age: Duration,
}

/// The sources a resolved leg's value is composed of, in non-increasing weight
/// order (see [`Contributors::iter`] for how ties are settled).
///
/// Empty whenever the leg resolved to nothing — there is no such thing as a
/// contributor to an absent value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Contributors {
    slots: [Option<Contributor>; MAX_CANDIDATES],
}

impl Contributors {
    /// Nothing composed the value, because there is no value.
    fn none() -> Self {
        Self::default()
    }

    /// A single source that *is* the answer: a designated source anchoring
    /// alone, a lone source, or the middle of an odd-sized median. The
    /// singleton case — and the only shape in which single-name attribution is
    /// truthful.
    fn one(c: Candidate) -> Self {
        let mut set = Self::default();
        set.slots[0] = Some(Contributor {
            source: c.source,
            weight: 1.0,
            age: c.reading.age,
        });
        set
    }

    /// Two sources averaged into the answer: an even-sized median's middle
    /// pair, of which an agreeing pair is the two-source case.
    fn pair(lo: Candidate, hi: Candidate) -> Self {
        let mut set = Self::default();
        for (slot, c) in set.slots.iter_mut().zip([lo, hi]) {
            *slot = Some(Contributor {
                source: c.source,
                weight: 0.5,
                age: c.reading.age,
            });
        }
        set
    }

    /// Every contributor, in non-increasing weight order.
    ///
    /// Equal weights — which is every multi-member set today, since the only
    /// shared case is an even median's `0.5`/`0.5` — are ordered by **value
    /// ascending**, and equal values keep offer order. Deterministic, so a
    /// consumer rendering the set gets a stable order tick to tick; but it is
    /// a tie-break, not a ranking, and nothing may read the first element as
    /// the dominant one. [`Contributors::dominant`] is the accessor that
    /// answers that honestly.
    pub fn iter(&self) -> impl Iterator<Item = &Contributor> {
        self.slots.iter().flatten()
    }

    /// How many sources the value is composed of. Never more than two today,
    /// since a median draws on its middle only — but read it rather than
    /// assuming that, which is a property of the resolution rule and not of
    /// this type.
    pub fn len(&self) -> usize {
        self.iter().count()
    }

    /// Whether the leg resolved to nothing.
    pub fn is_empty(&self) -> bool {
        self.iter().next().is_none()
    }

    /// The one source that *is* this leg's value, when there is one.
    ///
    /// `Some` only for a singleton set. Deliberately `None` for an averaged
    /// pair rather than picking a half of an exact tie: that is precisely the
    /// case where "the tier that answered" has no truthful answer, and
    /// returning one anyway is the lie this type exists to retire. A consumer
    /// wanting a single-name column should render that `None` as null, not
    /// reach into [`Contributors::iter`] for the first element.
    pub fn dominant(&self) -> Option<Contributor> {
        match (self.slots[0], self.slots[1]) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    }
}

impl Consensus {
    /// The resolution of a leg with no healthy source.
    pub fn absent() -> Self {
        Self {
            reading: None,
            state: ConsensusState::Absent,
            contributors: Contributors::none(),
            outlier: None,
            n: 0,
            healthy: [None; MAX_CANDIDATES],
        }
    }

    /// Every healthy candidate, tape and reference class alike — what the
    /// fusion estimator consumes.
    ///
    /// Returned as the backing slice rather than an iterator so the fusion can
    /// take it without collecting: a fixed array is what keeps this crate
    /// allocation-free, and handing out an iterator would push every consumer
    /// into a `Vec` to get a slice back.
    pub fn healthy(&self) -> &[Option<Candidate>] {
        &self.healthy
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
    /// Which sources the value is composed of, and in what proportion —
    /// **empty whenever [`Consensus::reading`] is `None`**.
    ///
    /// This is attribution, and it is deliberately a *set* rather than a single
    /// name (see [`Contributor`]). Every case is exact, and **the first row
    /// that applies wins**:
    ///
    /// | resolution | contributors |
    /// |---|---|
    /// | a designated source that is not the outlier | that source at `1.0` |
    /// | a lone source, designated or not | that source at `1.0` |
    /// | median, odd count | the middle source at `1.0` |
    /// | median, even count | the two middle sources at `0.5` each |
    /// | an agreeing pair | both at `0.5` — the even case with two |
    /// | a dispersed pair, no designation surviving | none; the leg resolves to nothing |
    ///
    /// **The first row is a precedence rule, not a special case**, and reading
    /// it as "only when the source is alone" is the trap: a designation anchors
    /// whether or not anything corroborates it, so it overrides every median
    /// and pair row below. A designated member of an agreeing pair takes `1.0`,
    /// not `0.5`; a designated source among three takes `1.0` even though a
    /// median exists. The one thing that displaces it is being **contradicted**
    /// — a designation that is itself the outlier is dropped, and the rows
    /// below then apply.
    ///
    /// A dispersed set of three or more still yields contributors, because
    /// either a surviving designation anchors it or the median stands — that is
    /// what the robustness buys. The members outside the middle bound the
    /// answer without entering it, so they carry no weight and do not appear
    /// here; [`Consensus::n`] is the count that includes them.
    ///
    /// Note what this is **not**: it names the sources to believe, where
    /// [`Consensus::outlier`] names the one to distrust. Reading either as the
    /// other is exactly backwards.
    pub contributors: Contributors,
    /// The source furthest from the consensus, when the set is dispersed.
    pub outlier: Option<&'static str>,
    /// How many healthy sources were **judged** by the fast consensus.
    ///
    /// It is bounded on both sides by things it is not, and both bounds are
    /// load-bearing:
    ///
    /// - It is **not** the number credited in [`Consensus::contributors`]. A
    ///   median's outer members are counted here and carry no weight there, so
    ///   the two legitimately differ.
    /// - It is **not** the size of [`Consensus::healthy`] below. Only the
    ///   tape-class set is judged (or the whole healthy set on a leg with no
    ///   tape source), so a reference fix that was fused but kept out of the
    ///   median did not corroborate the fast signal, and counting it here would
    ///   say it did.
    pub n: usize,
    /// Every healthy candidate offered for the leg, of either class, in offer
    /// order. The fusion estimator's input — see [`Consensus::healthy`].
    healthy: [Option<Candidate>; MAX_CANDIDATES],
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

/// The contributors a median is composed of, given the set ranked by value.
///
/// The mirror of [`median`], and it has to stay one: that function computes the
/// value, this one names who produced it, and the two read the same middle of
/// the same ranking. The members outside the middle bound the answer without
/// entering it — which is exactly the robustness a median buys — so they carry
/// no weight and do not appear.
///
/// Reached only from [`Candidates::resolve`], where the healthy set is
/// **compacted into its first `n` slots** and `n >= 2`, so neither `None` arm
/// below can fire. They return an under-credited set rather than indexing
/// because a panic on the quoting hot path would be the worse failure — but the
/// packing is the actual guarantee here, not the fallback.
fn middle_of(ranked: &[Option<Candidate>]) -> Contributors {
    let n = ranked.len();
    let Some(hi) = ranked.get(n / 2).copied().flatten() else {
        return Contributors::none();
    };
    if n % 2 == 1 {
        return Contributors::one(hi);
    }
    // `n` is even and non-zero here, so `n / 2 - 1` cannot underflow.
    match ranked.get(n / 2 - 1).copied().flatten() {
        Some(lo) => Contributors::pair(lo, hi),
        None => Contributors::one(hi),
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

/// The other source in a pair — the one that is not `source`.
///
/// Only meaningful for a two-source set, where it answers "which of these two
/// is the suspect?" from the designation rather than from a distance comparison
/// that cannot separate them.
fn partner_of(healthy: &[Option<Candidate>], source: &'static str) -> &'static str {
    healthy
        .iter()
        .flatten()
        .find(|c| c.source != source)
        .map_or(source, |c| c.source)
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
        assert_eq!(c.healthy().iter().flatten().count(), 0);
    }

    /// The operator ruling, as a test: a slow reference fix stays out of the
    /// fast median that guards dislocations, and stays in the healthy set the
    /// fusion estimator consumes.
    #[test]
    fn a_reference_fix_is_fused_but_never_drags_the_fast_median() {
        let c = Candidates::none()
            .push("oanda", Some(r(1.140)))
            .push("twelvedata", Some(r(1.142)))
            // Hours stale in substance, though still inside the freshness bound:
            // the case the exclusion exists for.
            .push_reference("frankfurter", Some(r(1.100)))
            .resolve(STALE, BAND);

        // The median is of the two tape sources alone — 1.141, not 1.140.
        assert!((c.reading.unwrap().value - 1.141).abs() < 1e-12);
        assert_eq!(c.n, 2, "the fix corroborates nothing about the fast signal");
        assert_eq!(c.state, ConsensusState::Agreed);
        assert!(
            !c.state.is_dispersed(),
            "and it cannot disperse a leg it is not in"
        );

        // But it is offered to the estimator, which is the other half of the
        // ruling — its information content is kept, not discarded.
        assert_eq!(c.healthy().iter().flatten().count(), 3);
        assert!(c
            .healthy()
            .iter()
            .flatten()
            .any(|k| k.source == "frankfurter"));

        // The two attributions of this one leg-tick disagree, and that is the
        // contract rather than a defect. The consensus contributor set describes
        // the fast combination, so a reference fix cannot appear in it — it was
        // never in the set that was combined. The fusion's own attribution does
        // credit it, at a weight its age discounts.
        assert!(
            !c.contributors.iter().any(|k| k.source == "frankfurter"),
            "a fix contributes nothing to a combination it was not part of"
        );
        assert_eq!(
            c.contributors.iter().count(),
            2,
            "exactly the two tape sources composed the median"
        );
    }

    /// The fallback that keeps reference-anchored markets alive: with no tape
    /// source at all, the reference candidates resolve the leg rather than
    /// darking it. Excluding a convention is about refusing to *pool* two of
    /// them, and a leg with only one present is not pooling anything.
    #[test]
    fn a_leg_with_only_reference_sources_still_resolves() {
        let c = Candidates::none()
            .push_reference("frankfurter", Some(r(1.14)))
            .resolve(STALE, BAND);

        assert_eq!(c.state, ConsensusState::SingleUnverified);
        assert_eq!(c.reading.unwrap().value, 1.14);
        assert_eq!(c.n, 1);
    }

    /// Two reference sources with no tape between them resolve against each
    /// other normally — the fallback is the whole reference set, not just the
    /// first of them.
    #[test]
    fn reference_only_legs_corroborate_each_other() {
        let c = Candidates::none()
            .push_reference("frankfurter", Some(r(1.140)))
            .push_reference("er-api", Some(r(1.142)))
            .resolve(STALE, BAND);

        assert_eq!(c.state, ConsensusState::Agreed);
        assert_eq!(c.n, 2);
    }

    /// A stale reference candidate drops out of the healthy set like any other,
    /// so the fusion never sees a reading the freshness bound rejected.
    #[test]
    fn an_expired_reference_is_not_offered_to_the_fusion() {
        let c = Candidates::none()
            .push("oanda", Some(r(1.140)))
            .push_reference("frankfurter", Some(Reading::new(1.100, secs(9_000))))
            .resolve(STALE, BAND);

        assert_eq!(c.healthy().iter().flatten().count(), 1);
        assert_eq!(c.n, 1);
    }

    /// Class and trust are independent axes: a reference source may still be
    /// designated believable alone, and doing so must not smuggle it into the
    /// fast median.
    #[test]
    fn a_trusted_reference_still_stays_out_of_the_fast_median() {
        let c = Candidates::none()
            .push("oanda", Some(r(1.140)))
            .push("twelvedata", Some(r(1.142)))
            .with(Some(Candidate::reference("ecb", r(1.100)).believed_alone()))
            .resolve(STALE, BAND);

        assert_eq!(c.n, 2);
        assert!((c.reading.unwrap().value - 1.141).abs() < 1e-12);
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
    fn the_pair_tie_break_does_not_depend_on_exact_float_representation() {
        // The case above passes for the wrong reason if the outlier is inferred
        // from a distance comparison: with two sources the median is their
        // midpoint, so the two distances are equal ONLY when `a + b` is exactly
        // representable. `1.14 / 1.05` happens to tie, which hid this. These
        // pairs do not tie — for the first, the *trusted* reading is
        // arithmetically the further one from the midpoint — so a
        // distance-inferred outlier names the designated source, drops the
        // anchor, and darks the leg.
        for (trusted, other) in [
            (1.675_697_883_552_159, 0.954_969_089_118_391_2),
            (1.14, 0.60),
            (0.60, 1.14),
        ] {
            let c = Candidates::none()
                .push_trusted("pyth-hermes", Some(r(trusted)))
                .push("frankfurter", Some(r(other)))
                .resolve(STALE, BAND);
            assert_eq!(c.state, ConsensusState::Dispersed);
            assert_eq!(
                c.outlier,
                Some("frankfurter"),
                "the source with no designation is the suspect ({trusted} vs {other})"
            );
            assert_eq!(
                c.reading.map(|x| x.value),
                Some(trusted),
                "the designated source must survive ({trusted} vs {other})"
            );
        }
    }

    #[test]
    fn an_overflowing_median_is_not_reported_as_agreement() {
        // With an even count the median averages the two middle values, so a
        // pair near the top of the range overflows to infinity — and dividing
        // the spread by infinity yields zero, which would report the widest
        // possible disagreement as perfect agreement.
        let c = Candidates::none()
            .push("a", Some(r(f64::MAX)))
            .push("b", Some(r(f64::MAX / 3.0)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert!(
            c.reading.is_none(),
            "a set that cannot be summarized has nothing to offer"
        );
        assert!(c.outlier.is_some(), "and the operator is told which source");
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

    /// The contributor set as `(source, weight)`, in iteration order — which is
    /// deliberately *not* a ranking; see [`Contributors::dominant`].
    fn credits(c: &Consensus) -> Vec<(&'static str, f64)> {
        c.contributors
            .iter()
            .map(|x| (x.source, x.weight))
            .collect()
    }

    #[test]
    fn a_lone_source_is_credited_in_full_whether_or_not_it_is_designated() {
        // Corroboration is `state`'s business. The contributor set answers a
        // different question — where the number came from — and for a lone
        // source that answer is the same either way.
        let unverified = Candidates::none()
            .push("coingecko", Some(r(1.14)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&unverified), [("coingecko", 1.0)]);

        let designated = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.14)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&designated), [("pyth-hermes", 1.0)]);
    }

    #[test]
    fn an_agreeing_pair_splits_its_weight_evenly() {
        // The value is their midpoint, so crediting either one alone would be
        // the ladder's "the tier that answered" told as a half-truth.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.142)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&c), [("coinbase", 0.5), ("kraken", 0.5)]);

        // Equal weights are ordered by value, not by the order offered, so a
        // consumer rendering the set sees the same order tick to tick however
        // the ladder happened to fill.
        let reversed = Candidates::none()
            .push("kraken", Some(r(1.142)))
            .push("coinbase", Some(r(1.140)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&reversed), [("coinbase", 0.5), ("kraken", 0.5)]);
    }

    #[test]
    fn an_odd_median_credits_only_its_middle() {
        // The outer two bound the answer without entering it — precisely the
        // robustness the median buys — so they carry no weight even though
        // `n` counts them.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.130)))
            .push("kraken", Some(r(1.140)))
            .push("coingecko", Some(r(1.145)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Corroborated, "within the band");
        assert_eq!(credits(&c), [("kraken", 1.0)]);
        assert_eq!(c.n, 3, "all three were judged, one was credited");
    }

    #[test]
    fn an_even_median_credits_its_middle_pair() {
        let c = Candidates::none()
            .push("coinbase", Some(r(1.130)))
            .push("kraken", Some(r(1.140)))
            .push("coingecko", Some(r(1.142)))
            .push("coinmarketcap", Some(r(1.145)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Corroborated, "within the band");
        assert_eq!(credits(&c), [("kraken", 0.5), ("coingecko", 0.5)]);
        assert_eq!(c.n, 4);
    }

    #[test]
    fn a_dispersed_even_set_still_credits_its_middle_pair() {
        // The even-count twin of `a_dispersed_majority_still_credits_its_median`.
        // Four or more survive one bad source exactly as three do, so the leg
        // keeps a usable value and must keep its attribution with it.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.130)))
            .push("kraken", Some(r(1.140)))
            .push("coingecko", Some(r(1.142)))
            .push("coinmarketcap", Some(r(1.400)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert_eq!(c.outlier, Some("coinmarketcap"));
        assert_eq!(credits(&c), [("kraken", 0.5), ("coingecko", 0.5)]);
    }

    #[test]
    fn credit_follows_value_order_not_offer_order() {
        // The middle is the middle of the *ranking*, so the same set offered
        // back-to-front must credit the same source. Ranking a copy rather
        // than `healthy` in place is what keeps this true without disturbing
        // the outlier tie-break.
        let forward = Candidates::none()
            .push("low", Some(r(1.130)))
            .push("mid", Some(r(1.140)))
            .push("high", Some(r(1.145)))
            .resolve(STALE, BAND);
        let reversed = Candidates::none()
            .push("high", Some(r(1.145)))
            .push("mid", Some(r(1.140)))
            .push("low", Some(r(1.130)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&forward), [("mid", 1.0)]);
        assert_eq!(credits(&reversed), [("mid", 1.0)]);
    }

    #[test]
    fn an_anchor_takes_the_whole_credit_rather_than_sharing_it() {
        // A designation says the source does not need the others' help, so the
        // attribution has to match the arithmetic: the others corroborated it
        // but did not enter the value.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.140)))
            .push("frankfurter", Some(r(1.142)))
            .push("coingecko", Some(r(1.141)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&c), [("pyth-hermes", 1.0)]);
        assert_eq!(c.reading.unwrap().value, 1.140);
    }

    #[test]
    fn a_designation_outranks_every_median_and_pair_rule() {
        // The precedence the contract table states, and the thing most likely
        // to be misread off it: a designation does not merely cover the
        // "anchoring alone" case, it OVERRIDES the median and pair rows. Both
        // sets below would credit differently under those rows, so each one
        // fails if the anchor arm ever stops being matched first.
        let agreeing_pair = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.140)))
            .push("frankfurter", Some(r(1.142)))
            .resolve(STALE, BAND);
        assert_eq!(agreeing_pair.state, ConsensusState::Agreed);
        assert_eq!(
            credits(&agreeing_pair),
            [("pyth-hermes", 1.0)],
            "not 0.5/0.5 — the designation anchors the pair"
        );
        assert_eq!(
            agreeing_pair.reading.unwrap().value,
            1.140,
            "not the midpoint"
        );

        // Four sources, so the even-median row would credit the middle PAIR.
        let even_median = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.130)))
            .push("frankfurter", Some(r(1.140)))
            .push("coingecko", Some(r(1.142)))
            .push("coinmarketcap", Some(r(1.145)))
            .resolve(STALE, BAND);
        assert_eq!(
            credits(&even_median),
            [("pyth-hermes", 1.0)],
            "not the middle pair at 0.5 each"
        );
        assert_eq!(even_median.n, 4, "all four judged, one credited");
    }

    #[test]
    fn a_designation_rescues_a_dispersed_pair_that_would_otherwise_go_dark() {
        // The empty-set row is scoped to a dispersed pair with NO designation,
        // and this is why. For a pair the outlier is `partner_of` the trusted
        // source — the un-designated one by construction — so the anchor
        // survives and the leg keeps a value instead of resolving to nothing.
        // Nothing else pins that, so a regression in the pair outlier rule
        // would silently dark this leg.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(1.140)))
            .push("frankfurter", Some(r(0.60)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert_eq!(c.outlier, Some("frankfurter"));
        assert_eq!(credits(&c), [("pyth-hermes", 1.0)]);
        assert_eq!(c.reading.unwrap().value, 1.140, "the leg is not dark");
    }

    #[test]
    fn an_anchored_leg_carries_only_the_anchors_age() {
        // The arm where the median's age rule does NOT apply. An anchor's
        // corroborators do not enter the value, and they do not enter its age
        // either — so a stale corroborator cannot age an anchored leg, which is
        // the same judgement as not letting one move its value.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(Reading::new(1.140, secs(2))))
            .push("frankfurter", Some(Reading::new(1.141, secs(120))))
            .resolve(STALE, BAND);
        assert_eq!(credits(&c), [("pyth-hermes", 1.0)]);
        assert_eq!(
            c.reading.unwrap().age,
            secs(2),
            "the anchor's own age, NOT the oldest healthy candidate's"
        );
    }

    #[test]
    fn a_contradicted_anchor_loses_its_credit_to_the_majority() {
        // The designation is overridden when the source is itself the outlier,
        // and the credit must follow the value rather than the designation —
        // otherwise the attribution would name the one source the resolver
        // just refused to believe.
        let c = Candidates::none()
            .push_trusted("pyth-hermes", Some(r(0.60)))
            .push("frankfurter", Some(r(1.140)))
            .push("coingecko", Some(r(1.141)))
            .resolve(STALE, BAND);
        assert_eq!(c.outlier, Some("pyth-hermes"));
        assert_eq!(credits(&c), [("frankfurter", 1.0)]);
    }

    #[test]
    fn a_leg_that_resolved_to_nothing_credits_nobody() {
        // Every shape with no reading: no source at all, a dispersed pair with
        // nothing to adjudicate it, and a median that overflowed to infinity.
        let absent = Candidates::none().resolve(STALE, BAND);
        let dispersed_pair = Candidates::none()
            .push("coinbase", Some(r(1.14)))
            .push("coingecko", Some(r(0.60)))
            .resolve(STALE, BAND);
        let overflowed = Candidates::none()
            .push("a", Some(r(f64::MAX)))
            .push("b", Some(r(f64::MAX / 2.0)))
            .resolve(STALE, BAND);

        for c in [absent, dispersed_pair, overflowed, Consensus::absent()] {
            assert!(c.reading.is_none());
            assert!(c.contributors.is_empty());
            assert_eq!(c.contributors.len(), 0);
            assert_eq!(c.contributors.dominant(), None);
        }
    }

    #[test]
    fn a_dispersed_majority_still_credits_its_median() {
        // Three or more survive one bad source, so the leg keeps a usable value
        // — flagged, not discarded. An empty contributor set here would say the
        // leg resolved to nothing, which is the opposite of what happened.
        let c = Candidates::none()
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.141)))
            .push("coingecko", Some(r(0.60)))
            .resolve(STALE, BAND);
        assert_eq!(c.state, ConsensusState::Dispersed);
        assert_eq!(c.outlier, Some("coingecko"));
        assert_eq!(credits(&c), [("coinbase", 1.0)]);
    }

    #[test]
    fn credited_weights_sum_to_one_whenever_the_leg_resolved() {
        // The invariant that makes the weights usable as weights: they are a
        // linear combination of contributor values, not a popularity score.
        let sets = [
            Candidates::none().push("a", Some(r(1.14))),
            Candidates::none()
                .push("a", Some(r(1.140)))
                .push("b", Some(r(1.142))),
            Candidates::none()
                .push("a", Some(r(1.130)))
                .push("b", Some(r(1.140)))
                .push("c", Some(r(1.145))),
            Candidates::none()
                .push("a", Some(r(1.130)))
                .push("b", Some(r(1.140)))
                .push("c", Some(r(1.142)))
                .push("d", Some(r(1.145))),
            Candidates::none()
                .push_trusted("a", Some(r(1.140)))
                .push("b", Some(r(1.142))),
        ];
        for set in sets {
            let c = set.resolve(STALE, BAND);
            assert!(c.reading.is_some());
            let total: f64 = c.contributors.iter().map(|x| x.weight).sum();
            assert!((total - 1.0).abs() < 1e-12, "weights summed to {total}");
        }
    }

    #[test]
    fn dominant_is_none_for_a_pair_rather_than_naming_half_of_it() {
        // The whole point of the type: a consumer wanting one name gets one
        // only when one is truthful. Picking a side of an exact 0.5/0.5 tie is
        // the ladder lie this replaces, so it must not be reachable through
        // this accessor.
        let pair = Candidates::none()
            .push("coinbase", Some(r(1.140)))
            .push("kraken", Some(r(1.142)))
            .resolve(STALE, BAND);
        assert_eq!(pair.contributors.len(), 2);
        assert_eq!(pair.contributors.dominant(), None);

        let single = Candidates::none()
            .push("coingecko", Some(r(1.14)))
            .resolve(STALE, BAND);
        assert_eq!(single.contributors.dominant().unwrap().source, "coingecko");
    }

    #[test]
    fn a_contributor_age_is_its_own_and_can_be_younger_than_the_leg() {
        // The distinction this field exists for, and the one that would
        // otherwise be misread. On a **median** the leg's age is the oldest
        // across every healthy candidate — including the outer members that
        // carry no weight — so it can exceed every credited age. (The anchor
        // arm behaves differently; `an_anchored_leg_carries_only_the_anchors_age`
        // pins that.) A staleness gate must keep reading the leg; these ages
        // only explain a contributor.
        let c = Candidates::none()
            .push("stale-outer", Some(Reading::new(1.130, secs(90))))
            .push("fresh-middle", Some(Reading::new(1.140, secs(1))))
            .push("outer", Some(Reading::new(1.145, secs(5))))
            .resolve(STALE, BAND);

        let credited = c.contributors.dominant().unwrap();
        assert_eq!(credited.source, "fresh-middle");
        assert_eq!(credited.age, secs(1), "the credited source's own age");
        assert_eq!(
            c.reading.unwrap().age,
            secs(90),
            "the leg stays as old as its worst input, credited or not"
        );
    }

    #[test]
    fn a_pair_carries_each_half_s_own_age() {
        let c = Candidates::none()
            .push("coinbase", Some(Reading::new(1.140, secs(2))))
            .push("kraken", Some(Reading::new(1.142, secs(40))))
            .resolve(STALE, BAND);
        let ages: Vec<_> = c.contributors.iter().map(|x| (x.source, x.age)).collect();
        assert_eq!(ages, [("coinbase", secs(2)), ("kraken", secs(40))]);
        assert_eq!(c.reading.unwrap().age, secs(40));
    }

    #[test]
    fn an_unhealthy_source_is_never_credited() {
        // The set is drawn from the healthy candidates, so a dead source cannot
        // appear in the attribution however it was offered.
        let c = Candidates::none()
            .push("dead", Some(Reading::new(1.14, secs(9_999))))
            .push("live", Some(r(0.99)))
            .resolve(STALE, BAND);
        assert_eq!(credits(&c), [("live", 1.0)]);
    }
}
