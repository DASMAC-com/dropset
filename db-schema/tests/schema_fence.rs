// cspell:word bytea
// cspell:word schemaname
// cspell:word tablename
// cspell:word unprovisioned
//! End-to-end tests for the startup fence against a real Postgres in a
//! throwaway container.
//!
//! The fence's whole job is to distinguish three states a live database can be
//! in relative to the binary talking to it — unprovisioned, current, and
//! *ahead* — and to fail on only one of them. The "ahead" case is the one
//! worth pinning: migrations are additive-only precisely so an old binary can
//! keep serving through a deploy that has already advanced the schema, so a
//! fence that demanded equality would turn a supported window into a crash
//! loop. That behavior is invisible to a unit test and easy to regress.
//!
//! These need a Docker daemon, so they are `#[ignore]`d and skipped by the
//! default test run. Run them with:
//!
//! ```sh
//! cargo test -p dropset-db-schema -- --ignored
//! ```

use dropset_db_schema::{connect, expected_version, migrate, require_schema};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};

/// Start a throwaway Postgres and return a connected pool, with **no** schema
/// applied — each test decides what state to put it in.
async fn start_pg() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default()
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url).await.expect("connect pool");
    (container, pool)
}

/// Record an applied migration at `version` without running any SQL, to put the
/// bookkeeping table into a state this build's history cannot itself produce.
async fn stamp_version(pool: &PgPool, version: i64, description: &str) {
    sqlx::query(
        "INSERT INTO _sqlx_migrations
             (version, description, installed_on, success, checksum,
              execution_time)
         VALUES ($1, $2, now(), TRUE, ''::bytea, 0)",
    )
    .bind(version)
    .bind(description)
    .execute(pool)
    .await
    .expect("stamp migration row");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_an_unprovisioned_database() {
    let (_pg, pool) = start_pg().await;

    // Nothing has run: there is not even a bookkeeping table, which the fence
    // must report as a missing schema rather than tripping over the absent
    // relation.
    let err = require_schema(&pool)
        .await
        .expect_err("an empty database must not pass the fence");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_accepts_a_migrated_database() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    require_schema(&pool)
        .await
        .expect("a freshly migrated database must pass the fence");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_accepts_a_database_ahead_of_this_build() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // The deploy window: the schema has moved on but this binary has not.
    // Additive-only migrations make that safe, so the fence must allow it.
    stamp_version(&pool, expected_version() + 1, "a later migration").await;

    require_schema(&pool)
        .await
        .expect("a database ahead of this build must pass the fence (>=, not ==)");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_a_database_behind_this_build() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // Roll the bookkeeping back to before this build's history: the tables are
    // real, but the recorded version no longer covers what the binary expects.
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("clear migration bookkeeping");
    stamp_version(&pool, expected_version() - 1, "an earlier migration").await;

    let err = require_schema(&pool)
        .await
        .expect_err("a database behind this build must not pass the fence");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

/// The bookkeeping table exists but records no successful migration.
///
/// This is the **most reachable** failure state, not a theoretical one: sqlx
/// creates `_sqlx_migrations` *before* applying the first migration, so a run
/// that dies partway through `0001` leaves the table present and **empty**. The
/// opening migration uses plain `CREATE TABLE`, so a database still carrying
/// tables from the retired per-app regimes fails in precisely that way.
///
/// The two halves are deliberately distinct, because only the first can
/// actually occur. On Postgres, sqlx's bookkeeping insert hardcodes
/// `success = TRUE` and the migration shares its transaction, so a failed
/// migration leaves **no row at all** rather than a false one — the
/// `success = FALSE` row exists for the backends that cannot run DDL inside a
/// transaction. So the empty-table case is the real partial-first-run state,
/// and the false-row case is defensive coverage of the query's `WHERE success`
/// filter.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn fence_rejects_a_database_with_no_successful_migration() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    // The reachable state: the table exists, but holds no rows, exactly as a
    // rolled-back first migration leaves it.
    sqlx::query("DELETE FROM _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("clear migration bookkeeping");
    let err = require_schema(&pool)
        .await
        .expect_err("an empty bookkeeping table must not pass the fence");
    assert!(
        err.to_string().contains("dropset-migrate"),
        "the error must name the fix, got: {err}"
    );

    // The defensive case: a row exists but is not successful, so the filtered
    // aggregate still reads NULL rather than that row's version.
    sqlx::query(
        "INSERT INTO _sqlx_migrations
             (version, description, installed_on, success, checksum,
              execution_time)
         VALUES (1, 'a failed attempt', now(), FALSE, ''::bytea, 0)",
    )
    .execute(&pool)
    .await
    .expect("stamp a failed migration row");

    let err = require_schema(&pool)
        .await
        .expect_err("a database with no successful migration must not pass");
    let msg = err.to_string();
    assert!(
        msg.contains("dropset-migrate"),
        "the error must name the fix, got: {msg}"
    );
}

/// Every table the migrations are supposed to create actually exists.
///
/// The fence tests only read `_sqlx_migrations`, and no consumer crate has a
/// Postgres integration test over its own tables, so nothing else would notice
/// a table lost from the squashed history — or from a future edit to it. This
/// asserts the shape mechanically so that fidelity stops depending on review
/// by eye.
///
/// The list spans the whole history rather than just the opening migration:
/// a later migration's table is exactly as easy to lose to a bad edit, and
/// keeping the assertion in one place is what makes "add your table here"
/// the obvious step when adding one.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn migrate_creates_every_expected_table() {
    let (_pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    for table in [
        // 0001_init
        "feed_cursors",
        "fill_events",
        "events",
        "takes",
        "market_stats",
        "indexer_cursor",
        "cex_prices",
        // 0003_maker_telemetry
        "maker_telemetry",
        "maker_legs",
        "feed_health",
        // 0004_spot_ticks
        "spot_ticks",
        // 0005_pyth_fx_feeds
        "pyth_fx_feeds",
        // 0006_fair_price_fusion
        "maker_leg_contributions",
    ] {
        let present: bool = sqlx::query_scalar("SELECT to_regclass($1) IS NOT NULL")
            .bind(table)
            .fetch_one(&pool)
            .await
            .expect("probe table");
        assert!(present, "migration did not create `{table}`");
    }

    // A migration that only *adds columns* creates no table, so the probe above
    // cannot see it at all — 0006 would pass this test having done nothing.
    // Additive columns therefore need their own assertion, and this is the
    // place: the same "add yours here" step, one list along.
    for (table, column) in [
        ("maker_legs", "fused_value"),
        ("maker_legs", "fused_sigma"),
        ("maker_legs", "fusion_step"),
        ("maker_legs", "fused_count"),
    ] {
        // Scoped to `public`, matching the `to_regclass` probe above: without it
        // a same-named table in any visible schema would satisfy this.
        let present: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                 SELECT 1 FROM information_schema.columns
                 WHERE table_schema = 'public'
                   AND table_name = $1
                   AND column_name = $2
             )",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&pool)
        .await
        .expect("probe column");
        assert!(present, "migration did not add `{table}.{column}`");
    }

    // The singleton watermark is seeded by the migration, not by the indexer,
    // so its absence would only surface as a missing row at aggregation time.
    let cursors: i64 = sqlx::query_scalar("SELECT count(*) FROM indexer_cursor")
        .fetch_one(&pool)
        .await
        .expect("count indexer_cursor rows");
    assert_eq!(
        cursors, 1,
        "indexer_cursor must be seeded with exactly one row"
    );
}

/// The shared reader role can read every table and cannot write any of them.
///
/// This is the only test in the suite that connects as a role other than the
/// owner, and it has to: `dropset_ro`'s value is entirely in what it is
/// *unable* to do, and every privilege here — the role's own existence, schema
/// `USAGE`, table `SELECT`, and the absence of `INSERT` — is invisible to a
/// connection that holds them all implicitly. Granting one grant too many
/// would leave every other test passing.
///
/// The write probe targets `market_stats` because every column but its primary
/// key carries a default or is nullable, so a one-column `INSERT` is valid SQL
/// for a role that *is* allowed to write — which is what makes a rejection
/// attributable to privileges rather than to a malformed statement.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn reader_role_can_read_everything_and_write_nothing() {
    let (pg, pool) = start_pg().await;
    migrate(&pool).await.expect("apply migrations");

    let port = pg
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://dropset_ro:dropset_ro@127.0.0.1:{port}/postgres");
    let reader = connect(&url)
        .await
        .expect("the reader role must exist and be able to log in");

    // Derived from the catalog rather than hardcoded. The expected-table
    // list belongs in `migrate_creates_every_expected_table`, where it *is*
    // the assertion; here it would only be a fixture, and a stale one — a
    // table added by a later migration would silently never be probed
    // while this test kept passing.
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")
            .fetch_all(&pool)
            .await
            .expect("list public tables");
    assert!(
        !tables.is_empty(),
        "no tables in `public` — the probe below would vacuously pass"
    );

    for table in &tables {
        sqlx::query(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&reader)
            .await
            .unwrap_or_else(|e| panic!("reader cannot SELECT from `{table}`: {e}"));
    }

    // The `ALTER DEFAULT PRIVILEGES` clause is the one grant nothing above
    // exercises: every table that exists was already covered by the blanket
    // `GRANT SELECT ON ALL TABLES`, so a migration that dropped the default
    // privileges would still pass. It is also the clause the migration's
    // comment leans on hardest, and the one a future change is most likely
    // to break. Create a table as the owner *after* migrating and confirm
    // the reader can read it without any further grant.
    sqlx::query("CREATE TABLE later_migration_table (id INT PRIMARY KEY)")
        .execute(&pool)
        .await
        .expect("create a table as the owner");
    sqlx::query("SELECT count(*) FROM later_migration_table")
        .fetch_one(&reader)
        .await
        .expect("default privileges must cover a table created after 0002");

    let err = sqlx::query("INSERT INTO market_stats (market) VALUES ('probe')")
        .execute(&reader)
        .await
        .expect_err("the reader role must not be able to write");

    // 42501 is `insufficient_privilege`. Asserting the code rather than the
    // message keeps this from passing on some unrelated failure — a syntax
    // error or a dropped table would also be an `Err`.
    let code = err
        .as_database_error()
        .and_then(|e| e.code())
        .map(|c| c.to_string());
    assert_eq!(
        code.as_deref(),
        Some("42501"),
        "expected a privilege rejection, got: {err}"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn migrate_is_idempotent() {
    let (_pg, pool) = start_pg().await;

    // The compose init step re-runs on every stack restart, so a second pass
    // over an up-to-date database has to be a no-op rather than an error on
    // an already-existing table.
    migrate(&pool).await.expect("apply migrations");
    migrate(&pool).await.expect("re-apply migrations");

    require_schema(&pool)
        .await
        .expect("fence passes after re-run");
}
