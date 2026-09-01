//! The roster-registration statement, run against a real Postgres.
//!
//! **Why this needs its own test rather than riding the schema fence.** The
//! statement in `queries/instrument_register.sql` is a runtime, string-typed
//! query loaded through `include_str!`, so nothing else in the repo executes it:
//! clippy cannot see inside it, there is no compile-time `DATABASE_URL` macro to
//! check it, and `db-schema`'s fence tests reach `instrument_registry` by
//! hand-written INSERT. That leaves a gap with a bad shape — a column typo or a
//! bind mismatch would pass lint and every test, then abort startup for all
//! seven collectors, because registration is deliberately fatal.
//!
//! It also pins the one behavior the statement promises and the hand-INSERT
//! tests cannot: `first_registered_at` survives re-registration while
//! `last_registered_at` advances. That pair is what distinguishes a row a
//! collector is still confirming from one left behind by a pair dropped from a
//! roster.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```

use dropset_db_schema::{connect, migrate};
use dropset_market_data::instruments::register;
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};

/// A throwaway Postgres with the schema applied.
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
    migrate(&pool).await.expect("apply migrations");
    (container, pool)
}

/// The (first, last) registration timestamps for one (source, product).
async fn stamps(pool: &PgPool, source: &str, product: &str) -> (i64, i64) {
    sqlx::query_as(
        "SELECT first_registered_at, last_registered_at FROM instrument_registry
             WHERE source = $1 AND product_id = $2",
    )
    .bind(source)
    .bind(product)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| panic!("read stamps for {source}/{product}: {e}"))
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn registering_a_roster_writes_one_row_per_product() {
    let (_pg, pool) = start_pg().await;

    let roster: Vec<String> = ["EUR-USD", "GBP-USD", "AUD-USD"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    register(&pool, "oanda", &roster)
        .await
        .expect("register a roster");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instrument_registry WHERE source = 'oanda'")
            .fetch_one(&pool)
            .await
            .expect("count rows");
    assert_eq!(rows, 3, "one row per product in the roster");

    // The dimension must classify what was just registered — the join key
    // agreeing is the whole point of registering under the writer's source.
    let class: String =
        sqlx::query_scalar("SELECT asset_class FROM instruments WHERE product_id = 'EUR-USD'")
            .fetch_one(&pool)
            .await
            .expect("read the class of a registered product");
    assert_eq!(class, "fx-pair");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn re_registering_preserves_first_and_advances_last() {
    let (_pg, pool) = start_pg().await;
    let roster = vec!["EURC-USDC".to_string()];

    register(&pool, "coinbase", &roster)
        .await
        .expect("register");
    let (first, last) = stamps(&pool, "coinbase", "EURC-USDC").await;
    assert_eq!(first, last, "a first registration stamps both the same");

    // Wind the stored row back, so the re-registration's own clock is
    // unambiguously later without the test having to sleep.
    sqlx::query(
        "UPDATE instrument_registry
             SET first_registered_at = 1000, last_registered_at = 1000
         WHERE source = 'coinbase' AND product_id = 'EURC-USDC'",
    )
    .execute(&pool)
    .await
    .expect("wind the row back");

    register(&pool, "coinbase", &roster)
        .await
        .expect("re-register the same roster");

    let (first_after, last_after) = stamps(&pool, "coinbase", "EURC-USDC").await;
    assert_eq!(
        first_after, 1000,
        "first_registered_at must survive a restart"
    );
    assert!(
        last_after > 1000,
        "last_registered_at must advance on re-registration, got {last_after}"
    );

    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM instrument_registry WHERE product_id = 'EURC-USDC'",
    )
    .fetch_one(&pool)
    .await
    .expect("count rows");
    assert_eq!(rows, 1, "re-registration upserts rather than duplicating");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_repeated_product_id_does_not_abort_the_statement() {
    // `ON CONFLICT DO UPDATE` raises "cannot affect row a second time" if one
    // statement touches a row twice, and registration is fatal by design — so
    // without the DISTINCT in the query this would abort collector startup with
    // an opaque Postgres error. No current caller can produce a duplicate, but
    // `register` explicitly documents defending the shape of a future caller
    // assembling ids some other way, and this is the other half of that.
    let (_pg, pool) = start_pg().await;
    let roster = vec!["EUR-USD".to_string(), "EUR-USD".to_string()];

    register(&pool, "twelvedata", &roster)
        .await
        .expect("a duplicated id must not abort the statement");

    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM instrument_registry WHERE source = 'twelvedata'")
            .fetch_one(&pool)
            .await
            .expect("count rows");
    assert_eq!(rows, 1, "the duplicate collapses to one row");
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn an_empty_roster_is_a_no_op() {
    // Unreachable from the collectors (`parse_roster` rejects an empty roster),
    // and the early return is what makes it a no-op rather than a statement
    // unnesting an empty array.
    let (_pg, pool) = start_pg().await;
    register(&pool, "kraken", &[])
        .await
        .expect("empty is a no-op");

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM instrument_registry")
        .fetch_one(&pool)
        .await
        .expect("count rows");
    assert_eq!(rows, 0);
}
