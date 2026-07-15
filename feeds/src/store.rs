//! The store (warehouse) sink: idempotent Postgres persistence behind a
//! framework-owned resumable cursor.

use crate::cursor::{Cursor, CursorStore};
use crate::record::Batch;
use crate::sink::Sink;
use anyhow::Result;
use async_trait::async_trait;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// Connect a pool sized for a single feed process. The connection string
/// decides local vs. Aurora (docs/data-feeds.md §1).
pub async fn connect(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new().max_connections(8).connect(url).await?;
    Ok(pool)
}

/// How a consumer turns its typed records into rows. The framework owns the
/// transaction and the cursor; the consumer owns the record → table mapping,
/// which it writes idempotently (`ON CONFLICT DO NOTHING`) inside the passed
/// transaction. This is the seam the indexer's `write_events` becomes when it
/// migrates (docs/data-feeds.md §2, §6).
#[async_trait]
pub trait StoreWriter: Send + Sync {
    type Record: Send + Sync;

    /// Persist a batch's records within `tx`. Return rows actually written
    /// (after `ON CONFLICT` dedup), for logging / metrics.
    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[Self::Record],
    ) -> Result<u64>;
}

/// The store sink: persist each batch in one transaction, then advance the
/// feed's cursor.
///
/// **Delivery is at-least-once.** The cursor is saved *after* the batch's
/// transaction commits, so a crash in between re-fetches the last window on
/// restart; the writer's idempotent upsert absorbs the duplicate
/// (docs/data-feeds.md §3).
pub struct StoreSink<W: StoreWriter> {
    pool: PgPool,
    writer: W,
    feed: String,
    cursors: PgCursorStore,
}

impl<W: StoreWriter> StoreSink<W> {
    /// Wire a store sink for `feed`, persisting records via `writer` and
    /// cursors into the framework's `feed_cursors` table on the same pool.
    pub fn new(pool: PgPool, feed: impl Into<String>, writer: W) -> Self {
        let cursors = PgCursorStore::new(pool.clone());
        Self {
            pool,
            writer,
            feed: feed.into(),
            cursors,
        }
    }
}

#[async_trait]
impl<W> Sink<W::Record> for StoreSink<W>
where
    W: StoreWriter,
    W::Record: Send + Sync,
{
    async fn handle(&mut self, batch: &Batch<W::Record>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        self.writer.write_batch(&mut tx, &batch.records).await?;
        tx.commit().await?;
        if let Some(cursor) = &batch.cursor {
            self.cursors.save(&self.feed, cursor).await?;
        }
        Ok(())
    }
}

/// The framework-owned [`CursorStore`], backed by the `feed_cursors` table.
#[derive(Clone)]
pub struct PgCursorStore {
    pool: PgPool,
}

impl PgCursorStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Create the `feed_cursors` table if absent (the embedded migration). A
    /// feed process runs this once at startup, or a shared migrate task runs it
    /// ahead of the feeds (docs/data-feeds.md §5).
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations").run(&self.pool).await?;
        Ok(())
    }
}

#[async_trait]
impl CursorStore for PgCursorStore {
    async fn load(&self, feed: &str) -> Result<Option<Cursor>> {
        let row: Option<(serde_json::Value,)> =
            sqlx::query_as(include_str!("../queries/cursor_get.sql"))
                .bind(feed)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(value,)| Cursor::from_json(value)))
    }

    async fn save(&self, feed: &str, cursor: &Cursor) -> Result<()> {
        sqlx::query(include_str!("../queries/cursor_set.sql"))
            .bind(feed)
            .bind(cursor.as_json())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
