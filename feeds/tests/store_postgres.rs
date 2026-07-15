//! End-to-end tests for the store (warehouse) path against a real Postgres in
//! a throwaway container: the embedded migration, the `feed_cursors` upsert,
//! and the store sink's write + cursor-advance, including idempotency.
//!
//! These need a Docker daemon, so they are `#[ignore]`d and skipped by the
//! default test run. Run them with:
//!
//! ```sh
//! cargo test -p dropset-feeds --features store -- --ignored
//! ```
//!
//! Wiring them into CI behind a Postgres service is a tracked follow-up.

#![cfg(feature = "store")]

use dropset_feeds::{
    connect, Batch, Cursor, CursorStore, PgCursorStore, Sink, StoreSink, StoreWriter,
};
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::{runners::AsyncRunner, ContainerAsync};

/// Start a throwaway Postgres and return a connected pool. The container is
/// returned so the caller keeps it alive for the test's duration.
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

#[tokio::test]
#[ignore = "requires a Docker daemon (Postgres testcontainer)"]
async fn cursor_store_round_trips_and_overwrites() {
    let (_pg, pool) = start_pg().await;
    let cursors = PgCursorStore::new(pool.clone());
    cursors.migrate().await.unwrap();

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
#[ignore = "requires a Docker daemon (Postgres testcontainer)"]
async fn store_sink_persists_batch_and_advances_cursor_idempotently() {
    let (_pg, pool) = start_pg().await;
    let cursors = PgCursorStore::new(pool.clone());
    cursors.migrate().await.unwrap();
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
