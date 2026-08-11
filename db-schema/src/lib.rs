//! The single schema owner for the shared `dropset` database
//! (docs/data-feeds.md §8).
//!
//! Two responsibilities, and deliberately no others:
//!
//! - [`migrate`] applies the ordered migrations in `./migrations`. The
//!   `dropset-migrate` binary is the only thing that calls it in a deployment
//!   — as a run-once step before any dependent process starts.
//! - [`require_schema`] is the read-only fence a **DB-primary** app calls at
//!   startup to assert the schema it was compiled against is present, failing
//!   with an actionable error instead of a bare `relation … does not exist`
//!   halfway through a run.
//!
//! Before this crate, three DDL regimes coexisted: the `feeds` framework
//! migrated its own `feed_cursors` table, the indexer ran a second
//! `sqlx::migrate!` from inside `Store::connect`, and the market-data
//! collector applied idempotent `CREATE TABLE IF NOT EXISTS` startup DDL
//! specifically to avoid a second migrator colliding with the first on the
//! shared `_sqlx_migrations` bookkeeping table. They never actually collided
//! only because each pointed at a different database. Consolidating onto one
//! instance is what forced the question, and this crate is the answer: one
//! ordered history, one writer of DDL.

use anyhow::{bail, Result};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// The embedded, ordered migration history for the shared database.
///
/// Baked in at compile time, which is what lets [`require_schema`] compare a
/// live database against the schema *this* binary expects without consulting
/// anything on disk.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// A pool for schema work. Two connections is plenty: the runner is a
/// short-lived one-shot and the fence is a single query.
pub async fn connect(url: &str) -> Result<PgPool> {
    Ok(PgPoolOptions::new().max_connections(2).connect(url).await?)
}

/// The highest migration version embedded in this build.
///
/// The `unwrap_or(0)` must stay unreachable, and the reason is sharper than it
/// looks: at `expected == 0` the fence's `version >= expected` arm is
/// *universally* true, so [`require_schema`] would **fail open** and admit any
/// database with a non-empty history — including one holding somebody else's
/// schema. An empty `./migrations` would therefore disable the fence rather
/// than trip it. `migration_versions_ascend` below asserts the embedded history
/// is non-empty, so that state fails the test suite instead of reaching
/// production; the `debug_assert!` restates it at the call site.
pub fn expected_version() -> i64 {
    let version = MIGRATOR.iter().map(|m| m.version).max().unwrap_or(0);
    debug_assert!(
        version > 0,
        "no migrations embedded — the fence would fail open"
    );
    version
}

/// Apply every migration the target database has not yet run. Idempotent, so
/// re-running against an up-to-date database is a no-op — which is what makes
/// it safe as a compose init step that restarts with the stack.
pub async fn migrate(pool: &PgPool) -> Result<()> {
    MIGRATOR.run(pool).await?;
    Ok(())
}

/// Assert the database's applied history covers what this build expects, and
/// return an actionable error if it does not.
///
/// **The comparison is `>=`, never `==`.** Migrations are additive-only
/// precisely so that during a deploy an old binary can keep serving against a
/// schema newer than the one it was compiled against; an equality fence would
/// turn that supported window into a crash loop. Only a database *behind*
/// this build is a failure.
///
/// Call this from **DB-primary** apps only — the indexer and the market-data
/// collectors, which have nothing to do without their tables. Do **not** wire
/// it into the maker's quote path or the TUI go-between: there Postgres is a
/// soft dependency by design, and a database that is unreachable — or behind
/// on its migrations — means degraded/fallback operation surfaced in
/// telemetry, never a refusal to start.
///
/// The check is a **high-water mark**, not set coverage: it asks whether the
/// applied history reaches this build's latest version, not whether each
/// embedded version is individually present. So a database whose history
/// diverged from this branch can satisfy the fence — two branches that each add
/// a `0002` produce the same version number, and a consumer from one would pass
/// against the other's schema and then fail at query time.
///
/// That residual gap is accepted rather than closed, but note precisely what
/// covers it: [`migrate`] validates each migration's checksum, so divergence
/// fails there with an exact error — **provided the applied history came from
/// this build**. Since `migrate` runs in a different process from this fence,
/// the guarantee holds only because the deployment runs both from one tree and
/// gates every consumer on that step. It is not a property of the fence itself.
pub async fn require_schema(pool: &PgPool) -> Result<()> {
    let expected = expected_version();

    // `to_regclass` resolves a name to an OID or NULL, so the
    // not-yet-initialized case is an ordinary NULL rather than an error to
    // pattern-match on by code.
    let initialized: bool =
        sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(pool)
            .await?;
    if !initialized {
        bail!(
            "the shared `dropset` database has no schema (expected migrations \
             through v{expected}); run `dropset-migrate` against it before \
             starting this process"
        );
    }

    let applied: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(pool)
            .await?;
    match applied {
        Some(version) if version >= expected => Ok(()),
        Some(version) => bail!(
            "the shared `dropset` database is at migration v{version}, but \
             this build expects v{expected}; run `dropset-migrate` to bring \
             it up to date"
        ),
        None => bail!(
            "the shared `dropset` database has no successfully applied \
             migrations (expected through v{expected}); run `dropset-migrate` \
             against it before starting this process"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded history must be non-empty and its versions strictly
    /// ascending — the property the fence's `max(version)` comparison relies
    /// on, and the one a hand-numbered new migration file can break.
    #[test]
    fn migration_versions_ascend() {
        let versions: Vec<i64> = MIGRATOR.iter().map(|m| m.version).collect();
        assert!(!versions.is_empty(), "no migrations embedded");
        assert!(
            versions.windows(2).all(|w| w[0] < w[1]),
            "migration versions must strictly ascend: {versions:?}"
        );
        assert_eq!(expected_version(), *versions.last().unwrap());
    }

    /// No migration may be empty: a zero-length file is almost always one
    /// that was created but never filled in, and it would silently advance
    /// the fence while creating nothing.
    #[test]
    fn migrations_are_non_empty() {
        for m in MIGRATOR.iter() {
            assert!(
                !m.sql.trim().is_empty(),
                "migration v{} ({}) is empty",
                m.version,
                m.description
            );
        }
    }
}
