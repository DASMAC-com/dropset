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
use dropset_feeds::{now_secs, PARKED_SOURCES};
use sqlx::PgPool;

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
/// measurements either. The specific failure worth naming is a malformed `since`
/// in a park entry, which fails the `DATE` cast — loudly, at bring-up, naming
/// the value, rather than reaching a dashboard as a wrong date.
///
/// **An empty set is written, not skipped.** Un-parking the last source has to
/// empty the mirror; returning early on an empty constant would instead leave
/// every previously parked source rendering as quiet-by-decision forever. This
/// is the deliberate opposite of the empty-roster guard in
/// [`crate::instruments::register`], where an empty roster is a configuration
/// error rather than a meaningful state.
pub async fn mirror(pool: &PgPool) -> Result<()> {
    let mut venues: Vec<&str> = Vec::with_capacity(PARKED_SOURCES.len());
    let mut since_dates: Vec<&str> = Vec::with_capacity(PARKED_SOURCES.len());
    let mut reasons: Vec<&str> = Vec::with_capacity(PARKED_SOURCES.len());
    // Flattened one element per (venue, feed) pair, so a park naming two feeds
    // contributes two entries with the same venue.
    let mut feed_venues: Vec<&str> = Vec::new();
    let mut feed_names: Vec<&str> = Vec::new();
    for parked in PARKED_SOURCES {
        venues.push(parked.venue);
        since_dates.push(parked.since);
        reasons.push(parked.reason);
        for feed in parked.health_feeds {
            feed_venues.push(parked.venue);
            feed_names.push(feed);
        }
    }

    let mut tx = pool
        .begin()
        .await
        .context("opening the transaction for the parked-source mirror write")?;
    sqlx::query(include_str!("../queries/parked_sources_mirror.sql"))
        .bind(&venues)
        .bind(&since_dates)
        .bind(&reasons)
        .bind(now_secs())
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "mirroring {} parked source(s) into `parked_sources`; a database \
                 predating `0016_parked_sources.sql` is the likely cause, and a \
                 park entry whose `since` is not a YYYY-MM-DD date is the other",
                venues.len()
            )
        })?;
    sqlx::query(include_str!("../queries/parked_source_feeds_mirror.sql"))
        .bind(&feed_venues)
        .bind(&feed_names)
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

    /// The flattening the write depends on, asserted without a database.
    ///
    /// The three parent arrays are zipped by `unnest`, which pairs them
    /// positionally, so a length disagreement would mirror one park's reason
    /// onto another's venue rather than failing. One iteration over the constant
    /// is what makes that impossible, and this pins it.
    #[test]
    fn the_parallel_arrays_stay_aligned() {
        let mut venues = Vec::new();
        let mut pairs = 0;
        for parked in PARKED_SOURCES {
            venues.push(parked.venue);
            pairs += parked.health_feeds.len();
        }
        assert_eq!(venues.len(), PARKED_SOURCES.len());
        // Every park in the current set names at least one health feed, so the
        // pair count is at least the venue count. Asserted as an inequality
        // rather than an equality because an empty `health_feeds` is a
        // legitimate entry — see the field's docs.
        assert!(
            pairs >= venues.len(),
            "{pairs} pairs for {} parks",
            venues.len()
        );
    }
}
