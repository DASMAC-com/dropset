//! Mirroring the parked-source set into reference data a panel can join.
//!
//! The near-twin of [`crate::instruments`]: both write, at startup, a fact the
//! environment owns into a table no measurement can produce. The difference is
//! whose fact it is. Instrument registration states *this* collector's roster,
//! so each process writes its own slice; a park is a platform-wide decision
//! carried in one compile-time constant, so every process writes the **whole**
//! set and the last one to start wins with the same content.
//!
//! **Why the collectors write it rather than the migration seeding it.** The
//! full argument is in `0016_parked_sources.sql` — read it there rather than
//! keeping two copies in step. The short of it: a seed would pin the set to
//! migration time, cost a migration per parking decision, and be uncorrectable
//! once applied.
//!
//! **Why every collector and not one designated writer.** A single writer would
//! need its own deploy unit — a compose service, an image `COPY` line and a
//! Makefile target — and this repository has already paid for that shape: er-api
//! sat wired into compose but reachable from neither the image nor any target,
//! so it never ran at all for weeks (`docs/dashboards.md` §8, item 3). Writing
//! from a path that already runs cannot be forgotten that way. The cost is that
//! a stack running no market-data collector never refreshes the mirror, which
//! `parked_sources.mirrored_at` makes legible rather than silent.
//!
//! **This is not a spawn gate.** Nothing reads these tables to decide whether
//! to start a tier — `dropset_feeds::parked_source` is what does that, at each
//! venue's own spawn site. A mirror that fell behind must never be able to start
//! a parked collector or stop a live one.

use anyhow::{Context, Result};
use dropset_feeds::{now_secs, ParkedSource, PARKED_SOURCES};
use sqlx::PgPool;

/// The parked set flattened into the parallel arrays the two statements bind.
///
/// A named struct rather than a tuple because `unnest` pairs these
/// **positionally**: a caller that swapped two same-typed arrays would mirror
/// one park's reason onto another's venue and fail nothing, so the field names
/// are the guard.
struct MirrorRows {
    venues: Vec<&'static str>,
    since_dates: Vec<&'static str>,
    reasons: Vec<&'static str>,
    /// One element per (venue, feed) pair, so a park naming two feeds appears
    /// twice here with the same venue.
    feed_venues: Vec<&'static str>,
    feed_names: Vec<&'static str>,
}

/// Flatten a parked set into those arrays.
///
/// Extracted from [`mirror_set`] so a test asserts the alignment against the
/// code the write actually uses, rather than against a second copy of this
/// loop — which is what a test that re-implements it would do, and it would
/// pass whatever this function did.
fn flatten_parked_set(parks: &[ParkedSource]) -> MirrorRows {
    let mut rows = MirrorRows {
        venues: Vec::with_capacity(parks.len()),
        since_dates: Vec::with_capacity(parks.len()),
        reasons: Vec::with_capacity(parks.len()),
        // At least one pair per park in the common case, and a park may
        // legitimately name none — so this is a floor, not the exact length.
        feed_venues: Vec::with_capacity(parks.len()),
        feed_names: Vec::with_capacity(parks.len()),
    };
    for parked in parks {
        rows.venues.push(parked.venue);
        rows.since_dates.push(parked.since);
        rows.reasons.push(parked.reason);
        for feed in parked.health_feeds {
            rows.feed_venues.push(parked.venue);
            rows.feed_names.push(feed);
        }
    }
    rows
}

/// Replace the mirrored parked-source set with this build's constant.
///
/// Idempotent, and safe to run concurrently with another collector doing the
/// same: both write identical content from the same compile-time constant, so
/// the only difference between two runs is `mirrored_at`.
///
/// **Both statements run in one transaction**, which is load-bearing in two
/// directions. A panel must never observe the parent rows replaced while the
/// feed names still describe the previous set — that intermediate state would
/// exclude the wrong feed from the staleness alert. And the child insert
/// references parents the same transaction wrote, so the parent statement has to
/// have landed first.
///
/// **A failure here fails startup**, matching [`crate::instruments::register`]
/// and for the same reason: Postgres is a hard dependency for a collector, so a
/// pool that cannot take these two small statements will not take the
/// measurements either.
///
/// **Be precise about the severity, because the parallel to
/// [`crate::instruments::register`] does not fully carry.** Registration is
/// fatal because the dashboards drive off the registry; nothing downstream
/// depends on the mirror being written, and a stale mirror is an explicitly
/// tolerated state. So the two failure modes that are properties of the shared
/// constant rather than of this process — a malformed `since`, and a venue
/// parked twice — would take every collector down at once for a reason
/// unrelated to any of their work. Both are therefore caught in
/// `feeds/src/parked.rs`'s own suite instead, as a red build. What remains
/// fatal here is a genuine database failure, which is the case the parallel
/// does carry.
///
/// **An empty set is written, not skipped.** Un-parking the last source has to
/// empty the mirror; returning early on an empty constant would instead leave
/// every previously parked source rendering as quiet-by-decision forever. This
/// is the deliberate opposite of the empty-roster guard in
/// [`crate::instruments::register`], where an empty roster is a configuration
/// error rather than a meaningful state.
pub async fn mirror(pool: &PgPool) -> Result<()> {
    mirror_set(pool, PARKED_SOURCES).await
}

/// Mirror an arbitrary parked set — the body of [`mirror`], with the set as a
/// parameter.
///
/// **This exists so the tests can be falsifiable, and that is not a
/// nicety.** `PARKED_SOURCES` currently holds one entry, so a suite driving
/// only the constant exercises exactly one shape: it can never reach the
/// empty set (where the prune must empty the mirror rather than skip the
/// write — the reason there is no early return), never reach two parks at
/// once (where the positional `unnest` zips could mis-pair), and never reach
/// a park that names no health feed (which the field docs call legitimate).
/// Worse, on an empty constant a suite driven only by it goes **vacuous** —
/// comparing empty to empty — so the writer could be deleted wholesale with
/// tests still green.
///
/// Production has exactly one caller and it passes `PARKED_SOURCES`.
pub async fn mirror_set(pool: &PgPool, parks: &[ParkedSource]) -> Result<()> {
    let rows = flatten_parked_set(parks);
    let MirrorRows {
        venues,
        since_dates,
        reasons,
        feed_venues,
        feed_names,
    } = &rows;

    let mut tx = pool
        .begin()
        .await
        .context("opening the transaction for the parked-source mirror write")?;
    sqlx::query(include_str!("../queries/parked_sources_mirror.sql"))
        .bind(venues)
        .bind(since_dates)
        .bind(reasons)
        .bind(now_secs())
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "mirroring {} parked source(s) into `parked_sources`; a park \
                 entry whose `since` is not a real YYYY-MM-DD date is the \
                 likely cause, and a venue parked twice is the other. (A \
                 database predating `0016_parked_sources.sql` fails earlier, \
                 at `require_schema`, so it is not this error.)",
                venues.len()
            )
        })?;
    sqlx::query(include_str!("../queries/parked_source_feeds_mirror.sql"))
        .bind(feed_venues)
        .bind(feed_names)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "mirroring {} parked feed-health name(s) into \
                 `parked_source_feeds`",
                feed_names.len()
            )
        })?;
    tx.commit()
        .await
        .context("committing the parked-source mirror write")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flattening the write depends on, asserted against the real function.
    ///
    /// The three parent arrays are zipped by `unnest`, which pairs them
    /// **positionally**, so a mis-ordered push would mirror one park's reason
    /// onto another's venue and fail nothing at all.
    ///
    /// This drives a THREE-entry fixture rather than `PARKED_SOURCES`, which
    /// holds one: with a single park, every ordering is trivially correct and
    /// any mis-pairing is invisible. The fixture also carries the two shapes
    /// the constant does not — a park naming two health feeds, and one naming
    /// none.
    #[test]
    fn the_parallel_arrays_stay_aligned() {
        let parks = [
            ParkedSource {
                venue: "alpha",
                since: "2026-01-01",
                reason: "first",
                health_feeds: &["alpha-one", "alpha-two"],
            },
            ParkedSource {
                venue: "beta",
                since: "2026-02-02",
                reason: "second",
                health_feeds: &[],
            },
            ParkedSource {
                venue: "gamma",
                since: "2026-03-03",
                reason: "third",
                health_feeds: &["gamma-one"],
            },
        ];

        let rows = flatten_parked_set(&parks);

        // Positional correspondence is the whole claim: index i of all three
        // parent arrays must describe the SAME park.
        assert_eq!(rows.venues, ["alpha", "beta", "gamma"]);
        assert_eq!(rows.since_dates, ["2026-01-01", "2026-02-02", "2026-03-03"]);
        assert_eq!(rows.reasons, ["first", "second", "third"]);

        // The child arrays flatten, so `beta` contributes nothing and `alpha`
        // contributes twice — which is why the pair count is neither the park
        // count nor a multiple of it.
        assert_eq!(rows.feed_venues, ["alpha", "alpha", "gamma"]);
        assert_eq!(rows.feed_names, ["alpha-one", "alpha-two", "gamma-one"]);
    }

    /// An empty set flattens to empty arrays rather than panicking or padding.
    ///
    /// This is the state the mirror write must still EXECUTE in — the prune
    /// empties the table — so the arrays it binds have to be well-formed when
    /// there is nothing to say.
    #[test]
    fn an_empty_set_flattens_to_nothing() {
        let rows = flatten_parked_set(&[]);
        assert!(rows.venues.is_empty());
        assert!(rows.since_dates.is_empty());
        assert!(rows.reasons.is_empty());
        assert!(rows.feed_venues.is_empty());
        assert!(rows.feed_names.is_empty());
    }
}
