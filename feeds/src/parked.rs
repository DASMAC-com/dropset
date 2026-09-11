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
//! **The consequence for a dashboard, which is not yet resolved.** Because the
//! set lives in code, a panel separating parked from faulted cannot join
//! against it — it either carries its own copy of the list, or a later change
//! seeds this set into reference data the panel can read.
//!
//! Parking a source rewrites no query, but it does change what one table
//! *contains*, and the difference matters on exactly the surfaces that render
//! liveness. A parked tier is not spawned, so nothing reports its health any
//! more: a `feed_health` row written before the park — keyed by the
//! **framework** name, `pyth-hermes` rather than the bare token below — is left
//! frozen with `last_ok_at` NULL, and the unfiltered `ok_age_secs > 1800` alert
//! keeps firing on it with nothing running that could ever clear it. Before the
//! park that row was self-healing: a credential arriving was enough. Retiring
//! it needs either an exclusion on the alert or a one-off delete, neither of
//! which lives here.
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
        }
    }
}
