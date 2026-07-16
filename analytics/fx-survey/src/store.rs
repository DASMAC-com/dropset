//! Postgres persistence for the survey: schema provisioning plus the
//! [`StoreWriter`] that maps [`Candle`] records onto `cex_prices`.
//!
//! The framework owns the `feed_cursors` table and its migration; this module
//! owns `cex_prices`. Rather than run a second `sqlx::migrate!` migrator — which
//! would collide with the framework's on the shared `_sqlx_migrations` table —
//! the schema is applied as idempotent `CREATE TABLE IF NOT EXISTS` DDL, the
//! pattern `feeds/tests/store_postgres.rs` sets (framework migrates its cursor
//! table; the consumer creates its own).

use crate::coinbase::Candle;
use anyhow::Result;
use async_trait::async_trait;
use dropset_feeds::{PgCursorStore, StoreWriter};
use sqlx::PgPool;

/// Provision every table the survey app needs: the framework's `feed_cursors`
/// (via its own migrator) and this app's `cex_prices` (idempotent DDL). Run
/// once by `fx-survey-migrate` before any feed starts (docs/data-feeds.md §5).
pub async fn migrate(pool: &PgPool) -> Result<()> {
    PgCursorStore::new(pool.clone()).migrate().await?;
    sqlx::raw_sql(include_str!("../schema/cex_prices.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

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
    ) -> Result<u64> {
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
