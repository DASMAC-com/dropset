//! Postgres persistence for the collector: the [`StoreWriter`] that maps
//! [`Candle`] records onto `cex_prices`.
//!
//! This module issues **no DDL**. Both tables it touches — the framework's
//! `feed_cursors` and this app's `cex_prices` — are defined in
//! `dropset-db-schema` and created by `dropset-migrate`, the single schema
//! owner (docs/data-feeds.md §8). The idempotent `CREATE TABLE IF NOT EXISTS`
//! this module used to run existed only to dodge a collision between two
//! `sqlx::migrate!` migrators on the shared `_sqlx_migrations` table; with one
//! owner there is nothing to dodge, and `cex_prices` is versioned like
//! everything else.

use async_trait::async_trait;
use dropset_feeds::{venues::Candle, StoreWriter};

/// Writes [`Candle`] records for one feed into `cex_prices`. The exchange,
/// pair, and granularity are constant per feed, so they live here and the
/// framework transaction + cursor advance come from [`dropset_feeds::StoreSink`].
pub struct CexWriter {
    source: String,
    product_id: String,
    granularity_secs: i32,
}

impl CexWriter {
    pub fn new(
        source: impl Into<String>,
        product_id: impl Into<String>,
        granularity_secs: i64,
    ) -> Self {
        Self {
            source: source.into(),
            product_id: product_id.into(),
            granularity_secs: granularity_secs as i32,
        }
    }
}

#[async_trait]
impl StoreWriter for CexWriter {
    type Record = Candle;

    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[Candle],
    ) -> anyhow::Result<u64> {
        let mut written = 0;
        for c in records {
            let res = sqlx::query(include_str!("../queries/cex_price_insert.sql"))
                .bind(&self.source)
                .bind(&self.product_id)
                .bind(self.granularity_secs)
                .bind(c.bucket_start)
                .bind(c.low)
                .bind(c.high)
                .bind(c.open)
                .bind(c.close)
                .bind(c.volume)
                .execute(&mut **tx)
                .await?;
            written += res.rows_affected();
        }
        Ok(written)
    }
}
