//! Shared harness for this crate's container-backed integration tests.
//!
//! Every test here needs the same thing: a Postgres nobody else is using, with
//! the schema applied. Three test binaries wanted it, which is one more than is
//! worth copying — and the copies had already started to drift in their panic
//! messages.
//!
//! **Never point these at the shared dev stack.** `start_pg` migrates, and the
//! tables involved hold immutable history: an applied migration is hashed by
//! the runner, so a schema advanced out from under the running stack is manual
//! surgery to undo. A throwaway container is the whole point.

// Cargo compiles this module separately into *every* integration-test binary
// that declares it, so a helper used by one of them is genuinely unused in the
// others and `-D dead-code` fails the build. The alternative — splitting the
// helpers so each binary declares only what it uses — trades one attribute for
// a file per consumer, which is the duplication this module removes.
#![allow(dead_code)]

use dropset_db_schema::{connect, migrate, POSTGRES_IMAGE_TAG};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};

/// A throwaway Postgres with every migration applied.
///
/// Keep the returned container bound for the life of the test — dropping it
/// stops the container, so binding it to `_` rather than `_pg` tears the
/// database down before the first query and fails in a way that looks like a
/// connection bug.
pub async fn start_pg() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default()
        .with_tag(POSTGRES_IMAGE_TAG)
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url).await.expect("connect pool");
    migrate(&pool).await.expect("apply migrations");
    (container, pool)
}

/// Insert one closed bucket into `cex_prices`.
///
/// `published_at` in the reader's terms is `bucket_start + granularity_secs`,
/// so this takes the close instant a test wants to assert on and works
/// backwards — a test writing `bucket_start` directly has to redo that
/// arithmetic at every call site and gets the off-by-one-bucket wrong.
pub async fn insert_bucket(
    pool: &PgPool,
    source: &str,
    product_id: &str,
    published_at: i64,
    close: f64,
) {
    const GRANULARITY: i64 = 60;
    sqlx::query(
        "INSERT INTO cex_prices
             (source, product_id, granularity_secs, bucket_start,
              low, high, open, close, volume)
         VALUES ($1, $2, $3, $4, $5, $5, $5, $5, 0)",
    )
    .bind(source)
    .bind(product_id)
    .bind(GRANULARITY as i32)
    .bind(published_at - GRANULARITY)
    .bind(close)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("insert {source}/{product_id} at {published_at}: {e}"));
}

/// Insert one spot print into `spot_ticks`.
///
/// The tick-table counterpart to [`insert_bucket`], and deliberately simpler:
/// `observed_at` needs no arithmetic because it is the stamp **as recorded** —
/// the venue's own publish time where the venue publishes one, else the
/// collector's poll second — where a bucket's publication instant is derived from
/// its start plus its width. Note that means a tick stamp is not always a
/// publication instant: the only `spot_ticks` venue that publishes one is Pyth,
/// so for the peg series it is a poll second, which is exactly the case the
/// reader's publication-versus-receipt `max()` exists to handle.
///
/// The confidence half-width is left NULL. The only venue that publishes one is
/// parked, and the tick reader deliberately does not project the column — so a
/// helper that took a confidence would offer tests a value nothing reads.
pub async fn insert_tick(
    pool: &PgPool,
    source: &str,
    product_id: &str,
    observed_at: i64,
    price: f64,
) {
    sqlx::query(
        "INSERT INTO spot_ticks (source, product_id, observed_at, price)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(source)
    .bind(product_id)
    .bind(observed_at)
    .bind(price)
    .execute(pool)
    .await
    .unwrap_or_else(|e| panic!("insert tick {source}/{product_id} at {observed_at}: {e}"));
}
