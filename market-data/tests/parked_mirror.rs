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
use dropset_feeds::{ParkedSource, PARKED_SOURCES};
use dropset_market_data::parked_mirror::{mirror, mirror_set};
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
async fn re_mirroring_refreshes_every_column_it_claims_to() {
    let (_pg, pool) = start_pg().await;
    mirror(&pool).await.expect("first mirror");

    // CLOBBER the stored row, then re-mirror. This is what makes the test
    // falsifiable, and the naive version — mirror twice and assert the stamp
    // did not go backwards — is not: `mirrored_at` is `now_secs()`, so two
    // mirrors inside one second are EQUAL, and `>=` holds even if the upsert's
    // whole `DO UPDATE SET` body is deleted. That would leave an operator's
    // edit to a park's date or reason silently never reaching the dashboard,
    // with four green tests. Writing a sentinel first means only a real
    // refresh can clear it.
    sqlx::query(
        "UPDATE parked_sources
             SET mirrored_at = 0, reason = 'clobbered', since = DATE '1970-01-01'",
    )
    .execute(&pool)
    .await
    .expect("clobber the stored row");

    // Every collector runs this at startup, so a restart — or nine collectors
    // starting at once — must not duplicate a row or fail on a conflict.
    mirror(&pool).await.expect("second mirror");

    // `since::TEXT` rather than a date type: Postgres renders a DATE as
    // `YYYY-MM-DD`, which is exactly the constant's own spelling, so the
    // comparison needs no date library and no `chrono` feature on sqlx.
    let rows: Vec<(String, i64, String, String)> = sqlx::query_as(
        "SELECT venue, mirrored_at, reason, since::TEXT FROM parked_sources
             ORDER BY venue",
    )
    .fetch_all(&pool)
    .await
    .expect("read refreshed rows");

    let mut expected: Vec<&str> = PARKED_SOURCES.iter().map(|p| p.venue).collect();
    expected.sort();
    assert_eq!(
        rows.iter().map(|(v, ..)| v.as_str()).collect::<Vec<_>>(),
        expected,
        "the venue set must not change across a re-mirror"
    );

    for (venue, mirrored_at, reason, since) in &rows {
        let park = PARKED_SOURCES
            .iter()
            .find(|p| p.venue == venue)
            .expect("a mirrored venue is in the constant");
        assert!(
            *mirrored_at > 0,
            "{venue}: mirrored_at was not refreshed, so the upsert's update \
             half is not running"
        );
        assert_eq!(reason, park.reason, "{venue}: reason was not refreshed");
        assert_eq!(since, park.since, "{venue}: since was not refreshed");
    }
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn un_parking_the_last_source_empties_the_mirror() {
    let (_pg, pool) = start_pg().await;

    // Two parks first, so the emptying is a real transition from a populated
    // mirror rather than a no-op against a fresh table — and so the multi-park
    // shape the constant cannot currently reach gets exercised end to end
    // against the real statements.
    let parks = [
        ParkedSource {
            venue: "alpha",
            since: "2026-01-01",
            reason: "parked for the test",
            health_feeds: &["alpha-hermes"],
        },
        ParkedSource {
            venue: "beta",
            since: "2026-02-02",
            reason: "also parked",
            health_feeds: &[],
        },
    ];
    mirror_set(&pool, &parks).await.expect("mirror two parks");
    assert_eq!(mirrored_venues(&pool).await, ["alpha", "beta"]);
    assert_eq!(
        mirrored_feeds(&pool).await,
        [("alpha".to_string(), "alpha-hermes".to_string())],
        "a park naming no health feed contributes no child row"
    );

    // Now un-park everything. This is the state the writer must still EXECUTE
    // in: an early return on an empty set would leave both previously-parked
    // sources rendering as quiet-by-decision forever, and would leave
    // `alpha-hermes` excluded from the staleness alert for a feed nobody parks
    // any more. It is also the state that makes every other test in this file
    // vacuous if it is ever reached by accident.
    mirror_set(&pool, &[]).await.expect("mirror the empty set");
    assert!(
        mirrored_venues(&pool).await.is_empty(),
        "un-parking every source must empty the parent table"
    );
    assert!(
        mirrored_feeds(&pool).await.is_empty(),
        "and must leave no exclusion behind"
    );
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
    // The child row goes with it. Note what this assertion does and does not
    // distinguish: the pair is absent from the incoming set too, so the child
    // statement's own prune would remove it even with no cascade. What the
    // cascade is actually pinned by is the FK itself — drop `ON DELETE
    // CASCADE` and the re-mirror above fails outright on a foreign-key
    // violation rather than reaching here.
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
    //
    // Pick a park that actually NAMES a health feed rather than index 0: an
    // empty `health_feeds` is legitimate, so a no-feed park landing first would
    // make the surviving-venue assertion below vacuous — it reads the child
    // table, where such a park has no row at all.
    let venue = PARKED_SOURCES
        .iter()
        .find(|p| !p.health_feeds.is_empty())
        .expect("a park naming at least one health feed")
        .venue;
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
