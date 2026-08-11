// cspell:word bytea
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
