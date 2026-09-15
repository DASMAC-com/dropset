//! End-to-end tests for the store path against a real Postgres in a
//! throwaway container: the `feed_cursors` upsert and the store sink's write +
//! cursor-advance, including idempotency.
//!
//! The framework no longer migrates anything, so these provision the schema
//! the way a deployment does — through `dropset-db-schema`, the single schema
//! owner (docs/data-feeds.md §8). That makes the test exercise the real
//! `feed_cursors` definition rather than a copy maintained beside it.
//!
//! These need a Docker daemon, so they are `#[ignore]`d and skipped by the
//! default test run. Run them with:
//!
//! ```sh
//! cargo test -p dropset-feeds --features store -- --ignored
//! ```
//!
//! CI runs them in the `Tests (Postgres)` job, which reaches the runner's
//! own Docker daemon rather than a `services:` Postgres — each test starts
//! and disposes of its own database, so they stay independent under
//! nextest's process-per-test parallelism.

#![cfg(feature = "store")]

use dropset_feeds::{
    connect, Batch, Cursor, CursorStore, PgCursorStore, Sink, StoreSink, StoreWriter,
};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt};

/// Start a throwaway Postgres and return a connected pool. The container is
/// returned so the caller keeps it alive for the test's duration.
async fn start_pg() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default()
        .with_tag(dropset_db_schema::POSTGRES_IMAGE_TAG)
        .start()
        .await
        .expect("start postgres container");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("resolve mapped port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = connect(&url).await.expect("connect pool");
    dropset_db_schema::migrate(&pool)
        .await
        .expect("apply shared schema");
    (container, pool)
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn cursor_store_round_trips_and_overwrites() {
    let (_pg, pool) = start_pg().await;
    let cursors = PgCursorStore::new(pool.clone());

    let feed = "cex:coinbase:EURC-USDC";
    // A feed that has never run has no cursor.
    assert!(cursors.load(feed).await.unwrap().is_none());

    let first = Cursor::from_json(serde_json::json!({ "next_start": 1_700_000_000u64 }));
    cursors.save(feed, &first).await.unwrap();
    assert_eq!(cursors.load(feed).await.unwrap(), Some(first));

    // Saving again overwrites in place (the upsert), not a second row.
    let second = Cursor::from_json(serde_json::json!({ "next_start": 1_700_000_060u64 }));
    cursors.save(feed, &second).await.unwrap();
    assert_eq!(cursors.load(feed).await.unwrap(), Some(second));
}

/// A minimal consumer writer: idempotent inserts into a test table.
struct PingWriter;

#[async_trait::async_trait]
impl StoreWriter for PingWriter {
    type Record = i64;

    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[i64],
    ) -> anyhow::Result<u64> {
        let mut written = 0;
        for id in records {
            let res = sqlx::query("INSERT INTO test_pings (id) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(*id)
                .execute(&mut **tx)
                .await?;
            written += res.rows_affected();
        }
        Ok(written)
    }
}

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn store_sink_persists_batch_and_advances_cursor_idempotently() {
    let (_pg, pool) = start_pg().await;
    let cursors = PgCursorStore::new(pool.clone());
    sqlx::query("CREATE TABLE test_pings (id BIGINT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();

    let feed = "test:pings";
    let mut sink = StoreSink::new(pool.clone(), feed, PingWriter);
    let cursor = Cursor::from_json(serde_json::json!({ "last_id": 3 }));
    let batch = Batch::new(vec![1i64, 2, 3]).with_cursor(cursor.clone());

    // First handle: records land and the cursor advances.
    sink.handle(&batch).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM test_pings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(cursors.load(feed).await.unwrap(), Some(cursor.clone()));

    // Re-handling the same batch is idempotent: the writer's ON CONFLICT
    // absorbs the duplicates (the at-least-once contract, docs/data-feeds.md
    // §3), so still three rows and the same cursor.
    sink.handle(&batch).await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM test_pings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(cursors.load(feed).await.unwrap(), Some(cursor));
}

/// The store sink records every batch's requested-vs-written counts, and the
/// schema's generated `outcome` separates the three states.
///
/// **The middle case is the point.** A fully-deduped re-handle writes zero rows
/// and is completely routine — it is what the at-least-once contract produces on
/// every crash-restart — so a "stored nothing" detector keyed on `written = 0`
/// would flag it. Only `empty_intake`, where nothing reached the writer at all,
/// is the state the filed symptom describes. This asserts the two are told
/// apart, since collapsing them is the way this table would silently stop
/// meaning anything.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn store_sink_records_the_three_batch_ingestion_outcomes() {
    let (_pg, pool) = start_pg().await;
    sqlx::query("CREATE TABLE test_pings (id BIGINT PRIMARY KEY)")
        .execute(&pool)
        .await
        .unwrap();

    let feed = "test:outcomes";
    let mut sink = StoreSink::new(pool.clone(), feed, PingWriter);

    // Every record is new: three offered, three written.
    sink.handle(&Batch::new(vec![1i64, 2, 3])).await.unwrap();
    // The same batch again: three offered, every one deduped away.
    sink.handle(&Batch::new(vec![1i64, 2, 3])).await.unwrap();
    // Nothing survived intake, which is the state worth distinguishing.
    sink.handle(&Batch::new(Vec::<i64>::new())).await.unwrap();
    // A partly-overlapping window: one new, one duplicate. Deliberately folded
    // into `stored` rather than given an outcome of its own — a partial dedup is
    // the ordinary result of an overlapping fetch and asks nothing of a reader.
    sink.handle(&Batch::new(vec![3i64, 4])).await.unwrap();

    let rows: Vec<(i64, i64, String)> = sqlx::query_as(
        "SELECT requested, written, outcome FROM feed_batch_ingestion \
         WHERE feed = $1 ORDER BY observed_at",
    )
    .bind(feed)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        rows,
        vec![
            (3, 3, "stored".to_string()),
            (3, 0, "all_duplicate".to_string()),
            (0, 0, "empty_intake".to_string()),
            (2, 1, "stored".to_string()),
        ]
    );
}

/// `outcome` is GENERATED, so a writer cannot state it — the discriminator is
/// the schema's to decide, not each caller's. Asserted because the column's
/// whole value over a plain `TEXT` is that it cannot drift from the counts, and
/// a later migration relaxing it would otherwise go unnoticed.
#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres container)"]
async fn batch_ingestion_outcome_cannot_be_written_by_hand() {
    let (_pg, pool) = start_pg().await;

    let err = sqlx::query(
        "INSERT INTO feed_batch_ingestion (feed, requested, written, outcome) \
         VALUES ('test:liar', 0, 0, 'stored')",
    )
    .execute(&pool)
    .await
    .expect_err("a generated column must reject a supplied value");

    // Postgres's wording for this is "cannot insert a non-DEFAULT value into
    // column", which names no "generated column" — asserted as the server
    // actually phrases it rather than as the feature is named.
    assert!(
        err.to_string().contains("non-DEFAULT value"),
        "expected a generated-column rejection, got: {err}"
    );
}
