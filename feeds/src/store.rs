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

/// A pool that defers connecting until its first query, for a consumer whose
/// database is a **soft** dependency.
///
/// [`connect`] establishes a connection eagerly, which is right for a
/// DB-primary app: it has nothing to do without its tables, so failing at
/// startup is the honest outcome. It is wrong for a telemetry writer, and the
/// failure is worse than it first looks — an eager connect that loses a
/// startup race against Postgres coming up does not merely retry later, it
/// leaves that process with **no** telemetry for its entire life, from one
/// transient error at second zero. Deferring means the first batch, and every
/// batch after it, gets a fresh attempt, so a consumer wrapped in
/// [`crate::BestEffortSink`] starts reporting whenever the database becomes
/// reachable and needs no ordering guarantee from whatever supervises it.
///
/// Only the URL is validated here, so a malformed connection string is still
/// an error at build time rather than a silent no-op. A URL that parses but
/// points nowhere is not caught — by design, since that is the case deferring
/// exists to tolerate.
///
/// **The short `acquire_timeout` is the other half of failing fast**, and
/// without it the laziness is half a fix: with the database unreachable each
/// batch would sit in `acquire()` for sqlx's 30-second default before the
/// best-effort wrapper could drop it, and a drain stalled 30 s per batch
/// back-pressures the channel feeding it — so an unreachable database would
/// surface as *dropped health updates* rather than as the one thing it is.
/// Two seconds is far longer than a healthy local or same-VPC acquire and far
/// shorter than a telemetry tick.
///
/// Four connections rather than [`connect`]'s eight: a single drain task
/// acquires one at a time, so this is sized for the one writer plus headroom,
/// not for a collector fleet.
pub fn connect_lazy(url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect_lazy(url)?;
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
        let written = self.writer.write_batch(&mut tx, &batch.records).await?;
        tx.commit().await?;
        if let Some(cursor) = &batch.cursor {
            self.cursors.save(&self.feed, cursor).await?;
        }
        tracing::debug!(feed = %self.feed, written, "store sink committed batch");
        Ok(())
    }
}

/// The framework-owned [`CursorStore`], backed by the `feed_cursors` table.
#[derive(Clone)]
pub struct PgCursorStore {
    pool: PgPool,
}

impl PgCursorStore {
    /// A cursor store over the given pool.
    ///
    /// The `feed_cursors` table must already exist: the framework no longer
    /// migrates it. It is defined in `dropset-db-schema` and created by
    /// `dropset-migrate`, the single schema owner (docs/data-feeds.md §8) —
    /// this type only reads and upserts rows. `feed_cursors` is §8's one
    /// ownership carve-out: several apps write it, partitioned by feed name,
    /// and the framework rather than any app defines its shape.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
