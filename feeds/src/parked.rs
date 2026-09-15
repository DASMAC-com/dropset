//! Sources configured out **by decision**, and the reason for each — so that a
//! source nobody is running reads as parked rather than as broken.
//!
//! **The failure this closes.** A dark source has two very different causes,
//! and nothing in the health tables distinguishes them: a collector that
//! crashed, and a collector deliberately not started. Both show no rows. The
//! operator's reading rule was prose — "pyth is dark by decision; every OTHER
//! dark source is a fault" — which decays the moment a second source is parked,
//! or a viewer who never read that sentence looks at a panel. Worse, the
//! by-decision case is not merely mislabelled: a tier that cannot succeed, if
//! spawned anyway, emits one `warn!` per poll forever (see [`crate::runner`]),
//! which is the noise that motivated this module.
//!
//! **Why this is a Rust constant and not a column on `instrument_registry`.**
//! That table is written only by a *running* collector, through
//! `register_instruments` — so a parked source can never write the row that
//! would say it is parked. The state cannot be represented there at all, which
//! is why it lives here rather than in the schema. It is also the honest home
//! on its own terms: being parked is a decision about deployment, not an
//! observation about data, and a decision belongs where it can be read next to
//! its reason.
//!
//! **What this set does NOT do, stated so nobody credits it with more.** An
//! entry is *inert* until the venue's own spawn site consults
//! [`parked_source`]. There is no central dispatcher that reads this table and
//! suppresses a tier; each call site opts in. So adding an entry for a venue
//! whose spawn site does not check it produces a source that is *described* as
//! parked and still runs — which is worse than not describing it at all. Add
//! the entry and the check together.
//!
//! **How a dashboard reaches this set, which used to be unresolved.** Because
//! the set lives in code, a panel separating parked from faulted cannot join
//! against it directly. The resolution is a **mirror**, not a move: the
//! market-data collectors replace the whole of this set in the
//! `parked_sources` / `parked_source_feeds` reference tables at startup
//! (`0016_parked_sources.sql`, written by `market-data/src/parked_mirror.rs`),
//! so a panel joins the tables while this constant stays the only place a park
//! is *decided*. Nothing reads those tables to decide whether to spawn a tier,
//! and nothing writes them by hand.
//!
//! Two consequences a reader should not have to derive. The mirror is only as
//! fresh as the last collector start, so a park added since then is real in
//! code and absent from SQL — `parked_sources.mirrored_at` is what says how
//! stale the answer is, and it is why a panel must never treat the mirror as
//! the authority on what is running. And because the write replaces the whole
//! set, removing an entry here deletes its rows at the next bring-up; no
//! migration is involved in parking or un-parking anything.
//!
//! Parking a source rewrites no query, but it does change what one table
//! *contains*, and the difference matters on exactly the surfaces that render
//! liveness. A parked tier is not spawned, so nothing reports its health any
//! more: a `feed_health` row written before the park — keyed by the
//! **framework** name, `pyth-hermes` rather than the bare token below — is left
//! frozen with `last_ok_at` NULL, and the unfiltered `ok_age_secs > 1800` alert
//! keeps firing on it with nothing running that could ever clear it. Before the
//! park that row was self-healing: a credential arriving was enough.
//!
//! Retiring that firing needs **both** halves, and they are different kinds of
//! thing. The alert and the maker feed-health panel now exclude any feed named
//! by [`ParkedSource::health_feeds`], which is the durable half and the reason
//! that field exists. The frozen row itself is **data**, so it goes by a
//! one-off `DELETE` an operator runs against the shared database — recorded in
//! `docs/data-feeds.md` §8, never as a migration, because a migration would
//! bake one database's accumulated state into the history every fresh database
//! then replays.
//!
//! **The venue token is the bare one**, matching
//! `instrument_source_liveness.source` (`pyth`, `oanda`, `kraken`) — *not* a
//! framework [`Source::name`](crate::Source::name), which is prefixed and
//! per-product for the per-product collectors (`cex:coinbase:EURC-USDC`), and
//! not a maker-bot fusion tag either. Those vocabularies are deliberately
//! separate and coincide only by accident, so a lookup keyed on the wrong one
//! silently matches nothing — the exact silent-join failure the schema catalog
//! warns about. Pass the bare token.
//!
//! For *this* adapter the second and third happen to be the same string:
//! `PythHermesSource::name()` and the maker bot's fusion tag are both
//! `pyth-hermes`, which is why the paragraph above can call it the framework
//! name and the maker bot's own constant can call it the fusion tag without
//! either being wrong. Neither is the bare token, which is the only thing that
//! matters here.

/// A source that is deliberately not running, and why.
///
/// The reason is a required field rather than an optional note: a park with no
/// recorded reason is indistinguishable from an abandoned one, and "why is this
/// off" is the whole question a reader arrives with. `since` dates the park so
/// a stale one is recognizable — a source parked long enough that nobody
/// intends to restore it is dead code, and deleting it beats designating it.
pub struct ParkedSource {
    /// The bare venue token, as `instrument_source_liveness.source` spells it.
    ///
    /// It must **also** be a compose service key, because that is what ties an
    /// entry to the deployment that holds the service back — which
    /// `feeds/tests/parked_compose_agreement.rs` asserts, while the bare-token
    /// shape itself is checked by `every_entry_is_well_formed` below. The two
    /// vocabularies are not the same one and only overlap by convention:
    /// `coinbase-ticker` is a compose service that is no venue token, so a park
    /// on a venue whose service is spelled differently needs an explicit
    /// mapping rather than this single field.
    pub venue: &'static str,
    /// `YYYY-MM-DD`, the date the source has been parked **since** — the date
    /// the condition began, not the date this entry was written.
    pub since: &'static str,
    /// Why it is parked, and what would un-park it.
    pub reason: &'static str,
    /// The `feed_health.feed` spellings this park silences — the **framework**
    /// names, not the bare [`venue`](Self::venue) token above.
    ///
    /// This field exists to keep a vocabulary bridge out of SQL. A park is
    /// decided against the bare token, while the health table and the staleness
    /// alert are keyed by framework name, so an exclusion written in SQL would
    /// have to relate the two vocabularies — the silent-join failure the schema
    /// catalog forbids, and unlike most such joins this one *looks* right,
    /// because for this adapter the two spellings differ by a suffix. Declaring
    /// the mapping here puts it where both spellings are known and leaves every
    /// query doing single-vocabulary equality.
    ///
    /// **An empty list is meaningful, not a default.** It says this park
    /// silences no health row — correct for a venue whose collector never wrote
    /// one. It is not the place to record uncertainty: an entry whose feed does
    /// write health rows, left empty here, goes on firing the staleness alert
    /// with nothing able to clear it, which is the exact defect the exclusion
    /// closes.
    ///
    /// **The over-broad direction is the dangerous one, and it is the reason
    /// each name must contain its venue token.** Too few names fails *open* —
    /// the alert keeps firing, which is noisy and safe. A name that matches a
    /// **running** venue's `feed_health.feed` fails *closed*: it removes that
    /// venue from the staleness alert and the maker feed-health panel
    /// permanently, and because the exclusion works by making a row absent,
    /// nothing renders the fact that anything was excluded. That is a live
    /// feed going dark with no signal anywhere. `every_entry_is_well_formed`
    /// below pins the containment check that catches a foreign or typo'd name;
    /// it cannot catch a name that is wrong *within* the same venue.
    ///
    /// The exclusion keys on **membership in this list**, never on a NULL
    /// `last_ok_at`. Never-answered is deliberately a firing state — a worse
    /// one than stopped-answering — so exempting NULL would blind the alert to
    /// every genuinely never-answered feed, and nothing in a diff would show
    /// it.
    pub health_feeds: &'static [&'static str],
}

/// Every source parked by decision.
///
/// **Removing an entry is how a source comes back**, and it restores the
/// previous behavior exactly: the tier spawns, polls, and reports health like
/// any other. Nothing else has to be touched.
///
/// Adding one is **not** symmetric — see the module docs. An entry only takes
/// effect where a spawn site consults [`parked_source`], so a new entry needs
/// its call site wired in the same change.
pub const PARKED_SOURCES: &[ParkedSource] = &[ParkedSource {
    venue: "pyth",
    since: "2026-08-26",
    // The pricing, and the open self-host-or-degrade decision this leaves
    // standing, are in docs/data-feeds.md §9. The rest of the rationale is in
    // `reason` below rather than repeated here, so the operator-visible copy
    // and the reader-visible copy cannot drift apart.
    reason: "Hermes went keyed in the Pyth Core upgrade and there is no usable \
             free tier, so no machine running this stack holds a credential; \
             kept for forensic use. Un-parked by a key or a self-hosted Hermes.",
    // `PythHermesSource::name()` and the maker bot's fusion tag are both this
    // string, which is what the maker wrote into `feed_health` while the tier
    // still ran — and what its frozen row is still keyed by.
    health_feeds: &["pyth-hermes"],
}];

/// The park record for `venue`, or `None` if it is expected to be running.
///
/// Takes the bare venue token — see the module docs on why passing a framework
/// source name or a fusion tag matches nothing instead of failing.
pub fn parked_source(venue: &str) -> Option<&'static ParkedSource> {
    PARKED_SOURCES.iter().find(|p| p.venue == venue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyth_is_parked_and_carries_its_reason() {
        let p = parked_source("pyth").expect("pyth is parked by decision");
        assert_eq!(p.since, "2026-08-26");
        assert!(
            p.reason.contains("keyed"),
            "the reason should name what changed: {}",
            p.reason
        );
    }

    #[test]
    fn a_running_source_is_not_parked() {
        // The negative matters as much as the positive: this lookup gates
        // whether a tier spawns at all, so a false positive darks a live feed.
        assert!(parked_source("kraken").is_none());
        assert!(parked_source("oanda").is_none());
        assert!(parked_source("coinbase").is_none());
    }

    /// The bare-token contract — see the module docs for why it matters. Pinned
    /// here so a caller keying on the wrong vocabulary fails a test rather than
    /// getting a silent `None`.
    #[test]
    fn the_key_is_the_bare_venue_token() {
        assert!(
            parked_source("pyth-hermes").is_none(),
            "that is a fusion tag"
        );
        assert!(
            parked_source("cex:pyth").is_none(),
            "that is a framework name"
        );
    }

    #[test]
    fn every_entry_is_well_formed() {
        for p in PARKED_SOURCES {
            assert!(!p.venue.is_empty(), "a park needs a venue");
            assert!(!p.reason.is_empty(), "{} needs a reason", p.venue);
            // The entry side of the bare-token contract, which the lookup tests
            // above cannot cover: they prove the wrong spelling does not MATCH,
            // not that the stored spelling is right. A framework name carries a
            // `:` and a fusion tag a `-`, so neither shape is a bare token.
            assert!(
                !p.venue.contains(':'),
                "{} looks like a framework name, not a bare venue token",
                p.venue
            );
            assert!(
                !p.venue.contains('-'),
                "{} looks like a fusion tag, not a bare venue token",
                p.venue
            );
            // Dated so a stale park is recognizable as one.
            assert_eq!(p.since.len(), 10, "{} since must be 10 characters", p.venue);
            assert!(
                p.since.chars().all(|c| c.is_ascii_digit() || c == '-'),
                "{} since must be digits and dashes only",
                p.venue
            );
            // The health-feed names are the OTHER vocabulary, and an empty
            // string would mirror a row matching nothing while reading as
            // coverage. The list itself may legitimately be empty — see the
            // field docs — so emptiness of the LIST is deliberately not
            // asserted here.
            for feed in p.health_feeds {
                assert!(
                    !feed.is_empty(),
                    "{} has an empty health feed name",
                    p.venue
                );
                // The containment guard the field docs promise. A name that
                // does not mention its own venue is either a typo or another
                // venue's feed, and the second case silences a RUNNING feed's
                // staleness alert with nothing rendering the exclusion. Not
                // `starts_with`: a framework name may be prefixed
                // (`cex:coinbase:EURC-USDC`), so containment is the strongest
                // form that holds for every shape.
                assert!(
                    feed.contains(p.venue),
                    "{}'s health feed {feed:?} does not name its own venue — a \
                     foreign name here silences a feed nobody parked",
                    p.venue
                );
            }
        }
    }

    /// Venues must be DISTINCT, and this is a database guard rather than a
    /// tidiness one.
    ///
    /// `parked_sources.venue` is the primary key and the mirror write upserts
    /// with `ON CONFLICT (venue) DO UPDATE`, which Postgres aborts with
    /// `ON CONFLICT DO UPDATE command cannot affect row a second time` when one
    /// statement presents the same key twice. The mirror write is fatal at
    /// collector startup, so a duplicated entry here would take down every
    /// market-data collector at the next bring-up — with a cardinality error
    /// that names neither the duplicate nor this file. Nothing else catches it:
    /// the character checks above are per entry, so a duplicate passes them
    /// twice.
    #[test]
    fn every_venue_is_distinct() {
        let mut seen: Vec<&str> = Vec::with_capacity(PARKED_SOURCES.len());
        for p in PARKED_SOURCES {
            assert!(
                !seen.contains(&p.venue),
                "{} is parked twice — the mirror's upsert cannot take one \
                 venue's key twice in a statement",
                p.venue
            );
            seen.push(p.venue);
        }
    }

    /// `since` must be a real calendar date, not merely date-SHAPED.
    ///
    /// The shape check in `every_entry_is_well_formed` accepts `2026-13-45`,
    /// and the mirror write casts this string to a Postgres `DATE` — so a
    /// transposed or out-of-range date passes every test here and then fails
    /// the cast at collector startup, fatally, for all nine collectors. Range
    /// checking it in the constant's own suite converts that fleet outage into
    /// a red build. Deliberately hand-rolled rather than pulling a date crate
    /// into this dependency-light module: what matters is the range, and the
    /// calendar's leap-year subtleties cannot change whether Postgres accepts
    /// these fields.
    #[test]
    fn every_since_is_a_real_date() {
        for p in PARKED_SOURCES {
            let parts: Vec<&str> = p.since.split('-').collect();
            assert_eq!(
                parts.len(),
                3,
                "{}'s since {:?} is not YYYY-MM-DD",
                p.venue,
                p.since
            );
            assert_eq!(parts[0].len(), 4, "{} needs a 4-digit year", p.venue);
            let month: u32 = parts[1].parse().expect("month parses");
            let day: u32 = parts[2].parse().expect("day parses");
            assert!(
                (1..=12).contains(&month),
                "{}'s since names month {month}",
                p.venue
            );
            assert!(
                (1..=31).contains(&day),
                "{}'s since names day {day}",
                p.venue
            );
        }
    }

    /// The bridge is only useful if it crosses vocabularies, so pin that it
    /// does. A `health_feeds` entry equal to the bare token is the shape a
    /// reader produces by filling the field in from the line above it, and it
    /// would silence nothing while looking complete — `feed_health` for this
    /// park is keyed `pyth-hermes`, so an exclusion on `pyth` matches no row.
    ///
    /// Stated as an assertion about *this* park rather than a blanket rule,
    /// because a venue whose framework name genuinely is its bare token is
    /// possible and would not be a defect.
    #[test]
    fn the_health_feed_name_is_the_framework_one() {
        let p = parked_source("pyth").expect("pyth is parked by decision");
        assert_eq!(
            p.health_feeds,
            &["pyth-hermes"],
            "the exclusion must key on what the maker wrote into feed_health"
        );
    }
}
