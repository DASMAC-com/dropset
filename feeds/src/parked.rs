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
//! is why it lives here rather than in the schema. It is also the honest
//! home on its own terms: being parked is a decision about deployment, not an
//! observation about data, and a decision belongs where it can be read next to
//! its reason.
//!
//! **The consequence for a dashboard, which is not yet resolved.** Because the
//! set lives in code, a panel separating parked from faulted cannot join
//! against it — it either carries its own copy of the list, or a later change
//! seeds this set into reference data the panel can read. Nothing here changes
//! what any existing query returns.
//!
//! **The venue token is the bare one**, matching
//! `instrument_source_liveness.source` (`pyth`, `oanda`, `kraken`) — *not* a
//! framework [`Source::name`](crate::Source::name), which is prefixed and
//! per-product for the per-product collectors (`cex:coinbase:EURC-USDC`), and
//! not a maker-bot fusion tag either (`pyth-hermes`). Those vocabularies are
//! deliberately separate and coincide only by accident, so a lookup keyed on
//! the wrong one silently matches nothing — the exact silent-join failure the
//! schema catalog warns about. Pass the bare token.

/// A source that is deliberately not running, and why.
///
/// The reason is a required field rather than an optional note: a park with no
/// recorded reason is indistinguishable from an abandoned one, and "why is this
/// off" is the whole question a reader arrives with. `since` dates the decision
/// so a park can be recognized as stale — a source parked long enough that
/// nobody intends to restore it is dead code, and deleting it beats designating
/// it.
pub struct ParkedSource {
    /// The bare venue token, as `instrument_source_liveness.source` spells it.
    pub venue: &'static str,
    /// `YYYY-MM-DD`, the date the decision was taken.
    pub since: &'static str,
    /// Why it is parked, and what would un-park it.
    pub reason: &'static str,
}

/// Every source parked by decision.
///
/// **Removing an entry is how a source comes back**, and it restores the
/// previous behavior exactly: the tier spawns, polls, and reports health like
/// any other. Nothing else has to be touched.
pub const PARKED_SOURCES: &[ParkedSource] = &[ParkedSource {
    venue: "pyth",
    since: "2026-08-26",
    // Hermes moved behind a bearer token in the Pyth Core upgrade, and Pyth
    // sells no usable free tier (docs/data-feeds.md §9), so no machine running
    // this stack holds a credential. Keyless it answers 401 on every poll.
    // Un-parked by a credential, or by a self-hosted Hermes — the open
    // self-host-or-degrade decision is in that same section.
    reason: "Hermes went keyed in the Pyth Core upgrade and there is no usable \
             free tier, so no machine running this stack holds a credential; \
             kept for forensic use. Un-parked by a key or a self-hosted Hermes.",
}];

/// The park record for `venue`, or `None` if it is expected to be running.
///
/// Takes the bare venue token — see the module docs on why passing a framework
/// source name or a fusion tag matches nothing instead of failing.
pub fn parked(venue: &str) -> Option<&'static ParkedSource> {
    PARKED_SOURCES.iter().find(|p| p.venue == venue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyth_is_parked_and_carries_its_reason() {
        let p = parked("pyth").expect("pyth is parked by decision");
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
        assert!(parked("kraken").is_none());
        assert!(parked("oanda").is_none());
        assert!(parked("coinbase").is_none());
    }

    /// The bare-token contract, pinned rather than left to the module docs. A
    /// framework `Source::name` or a fusion tag must NOT match, because a
    /// caller passing one would otherwise get a silent `None` that reads as
    /// "this source is fine".
    #[test]
    fn the_key_is_the_bare_venue_token() {
        assert!(parked("pyth-hermes").is_none(), "that is a fusion tag");
        assert!(parked("cex:pyth").is_none(), "that is a framework name");
    }

    #[test]
    fn every_entry_is_well_formed() {
        for p in PARKED_SOURCES {
            assert!(!p.venue.is_empty(), "a park needs a venue");
            assert!(!p.reason.is_empty(), "{} needs a reason", p.venue);
            // Dated so a stale park is recognizable as one.
            assert_eq!(p.since.len(), 10, "{} since is YYYY-MM-DD", p.venue);
            assert!(
                p.since.chars().all(|c| c.is_ascii_digit() || c == '-'),
                "{} since is YYYY-MM-DD",
                p.venue
            );
        }
    }
}
