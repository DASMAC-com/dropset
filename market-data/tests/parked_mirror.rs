//! The parked-source mirror write, run against a real Postgres.
//!
//! **Why this needs its own test.** Both statements in
//! `queries/parked_sources_mirror.sql` and
//! `queries/parked_source_feeds_mirror.sql` are runtime, string-typed queries
//! loaded through `include_str!`, so nothing else in the repo executes them —
//! the same gap `tests/instruments.rs` opens with, and the same consequence: a
//! column typo or a bind mismatch would pass lint and every other test, then
//! abort startup for all nine collectors, because the mirror write is
//! deliberately fatal.
//!
//! It also pins the behavior the fence manifest deliberately does not assert
//! with a `rows` directive. The rows are written by a collector rather than by
//! the migration, and what matters about them is not a count but that the
//! mirrored set *equals* `PARKED_SOURCES` after a write — including the two
//! prune paths, which are the only way an un-parked source stops rendering as
//! quiet-by-decision.
//!
//! Needs a Docker daemon, so `#[ignore]`d like the fence tests:
//!
//! ```sh
//! cargo test -p dropset-market-data -- --ignored
//! ```

use dropset_db_schema::{connect, migrate};
use dropset_feeds::PARKED_SOURCES;
use dropset_market_data::parked_mirror::mirror;
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

/// The mirrored venue tokens, in a stable order.
async fn mirrored_venues(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar("SELECT venue FROM parked_sources ORDER BY venue")
        .fetch_all(pool)
        .await
        .expect("read mirrored venues")
}

/// The mirrored (venue, feed) pairs, in a stable order.
async fn mirrored_feeds(pool: &PgPool) -> Vec<(String, String)> {
    sqlx::query_as("SELECT venue, feed FROM parked_source_feeds ORDER BY venue, feed")
        .fetch_all(pool)
        .await
        .expect("read mirrored feeds")
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn mirroring_writes_the_whole_constant() {
    let (_pg, pool) = start_pg().await;

    // A fresh database has no rows at all: the migration creates the tables and
    // seeds nothing, which is the shape that keeps parking free of migrations.
    assert!(
        mirrored_venues(&pool).await.is_empty(),
        "the migration must not seed the parked set"
    );

    mirror(&pool).await.expect("mirror the parked set");

    let mut expected: Vec<String> = PARKED_SOURCES.iter().map(|p| p.venue.to_string()).collect();
    expected.sort();
    assert_eq!(mirrored_venues(&pool).await, expected);

    let mut expected_feeds: Vec<(String, String)> = PARKED_SOURCES
        .iter()
        .flat_map(|p| {
            p.health_feeds
                .iter()
                .map(|f| (p.venue.to_string(), (*f).to_string()))
        })
        .collect();
    expected_feeds.sort();
    assert_eq!(mirrored_feeds(&pool).await, expected_feeds);
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn re_mirroring_is_idempotent_and_advances_only_the_stamp() {
    let (_pg, pool) = start_pg().await;
    mirror(&pool).await.expect("first mirror");
    let first: Vec<(String, i64)> =
        sqlx::query_as("SELECT venue, mirrored_at FROM parked_sources ORDER BY venue")
            .fetch_all(&pool)
            .await
            .expect("read first stamps");

    // Every collector runs this at startup, so a restart — or nine collectors
    // starting at once — must not duplicate a row or fail on a conflict.
    mirror(&pool).await.expect("second mirror");
    let second: Vec<(String, i64)> =
        sqlx::query_as("SELECT venue, mirrored_at FROM parked_sources ORDER BY venue")
            .fetch_all(&pool)
            .await
            .expect("read second stamps");

    assert_eq!(
        first.iter().map(|(v, _)| v).collect::<Vec<_>>(),
        second.iter().map(|(v, _)| v).collect::<Vec<_>>(),
        "the venue set must not change"
    );
    for ((_, before), (venue, after)) in first.iter().zip(second.iter()) {
        assert!(
            after >= before,
            "{venue}: mirrored_at went backwards, {before} to {after}"
        );
    }
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_source_no_longer_parked_is_pruned() {
    let (_pg, pool) = start_pg().await;
    mirror(&pool).await.expect("mirror the parked set");

    // Stand in for an un-parked source by planting a row the constant does not
    // declare. This is the state a database reaches by having been mirrored
    // before an entry was REMOVED from `PARKED_SOURCES`, which is exactly how
    // un-parking happens — and it cannot be produced by editing the constant
    // from a test.
    sqlx::query(
        "INSERT INTO parked_sources (venue, since, reason, mirrored_at)
             VALUES ('gone_venue', '2020-01-01', 'left over from an earlier park', 0)",
    )
    .execute(&pool)
    .await
    .expect("plant a stale parked row");
    sqlx::query(
        "INSERT INTO parked_source_feeds (venue, feed) VALUES ('gone_venue', 'gone_venue-feed')",
    )
    .execute(&pool)
    .await
    .expect("plant a stale feed row");

    mirror(&pool).await.expect("re-mirror");

    let venues = mirrored_venues(&pool).await;
    assert!(
        !venues.iter().any(|v| v == "gone_venue"),
        "an un-parked source must be pruned, got {venues:?}"
    );
    // The child row goes with it, via ON DELETE CASCADE rather than by being
    // named — which is what keeps a stale exclusion from outliving its park and
    // silencing a feed nobody parked.
    let feeds = mirrored_feeds(&pool).await;
    assert!(
        !feeds.iter().any(|(v, _)| v == "gone_venue"),
        "the cascade must take the feed rows too, got {feeds:?}"
    );
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn a_dropped_feed_name_is_pruned_while_its_venue_stays() {
    let (_pg, pool) = start_pg().await;
    mirror(&pool).await.expect("mirror the parked set");

    // The case the parent cascade cannot reach: the venue stays parked, so its
    // parent row survives and nothing cascades, but one of its health-feed
    // names is no longer declared. Without the child statement's own prune this
    // pair would survive and go on excluding a feed from the staleness alert.
    let venue = PARKED_SOURCES[0].venue;
    sqlx::query("INSERT INTO parked_source_feeds (venue, feed) VALUES ($1, 'stale-feed-name')")
        .bind(venue)
        .execute(&pool)
        .await
        .expect("plant a stale feed name");

    mirror(&pool).await.expect("re-mirror");

    let feeds = mirrored_feeds(&pool).await;
    assert!(
        !feeds.iter().any(|(_, f)| f == "stale-feed-name"),
        "a dropped feed name must be pruned, got {feeds:?}"
    );
    assert!(
        feeds.iter().any(|(v, _)| v == venue),
        "its venue must still be parked, got {feeds:?}"
    );
}
