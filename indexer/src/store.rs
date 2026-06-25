//! Postgres persistence: idempotent raw-event writes keyed on the event
//! PK, the aggregator's watermark + reads, and the `/v1` read queries.

use crate::model::{event_market, event_to_json, FillRow, MarketStatsRow, Take};
use crate::model::{DecodedEvent, EventCoords};
use dropset_sdk::events::DropsetEvent;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// The aggregator watermark: the last event coordinate folded.
#[derive(Clone, Copy, Debug, Default, sqlx::FromRow)]
pub struct Cursor {
    pub last_slot: i64,
    pub last_txn_index: i64,
    pub last_event_ordinal: i64,
}

/// One row of the JSONB fidelity table, for `/v1/events`.
#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct EventEnvelope {
    pub slot: i64,
    pub txn_index: i64,
    pub signature: String,
    pub event_ordinal: i64,
    pub block_time: Option<i64>,
    pub kind: String,
    pub market: Option<String>,
    pub payload: serde_json::Value,
}

#[derive(Clone)]
pub struct Store {
    pub pool: PgPool,
}

impl Store {
    /// Connect and run embedded migrations.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new().max_connections(8).connect(url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Persist a transaction's decoded events in one transaction. Fills go
    /// to the typed table; everything else to the JSONB fidelity table.
    /// Idempotent — `ON CONFLICT DO NOTHING` on the event PK.
    pub async fn write_events(&self, events: &[DecodedEvent]) -> anyhow::Result<u64> {
        let mut written = 0u64;
        let mut tx = self.pool.begin().await?;
        for de in events {
            match &de.event {
                DropsetEvent::Fill(f) => {
                    written += write_fill(&mut tx, &FillRow::from_event(&de.coords, f)).await?;
                }
                other => {
                    written += write_envelope(&mut tx, &de.coords, other).await?;
                }
            }
        }
        tx.commit().await?;
        Ok(written)
    }

    pub async fn cursor(&self) -> anyhow::Result<Cursor> {
        let c = sqlx::query_as::<_, Cursor>(
            "SELECT last_slot, last_txn_index, last_event_ordinal FROM indexer_cursor WHERE id = 1",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(c)
    }

    pub async fn set_cursor(&self, c: Cursor) -> anyhow::Result<()> {
        sqlx::query(
            "UPDATE indexer_cursor SET last_slot = $1, last_txn_index = $2, \
             last_event_ordinal = $3 WHERE id = 1",
        )
        .bind(c.last_slot)
        .bind(c.last_txn_index)
        .bind(c.last_event_ordinal)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fill legs with coordinates strictly after the cursor, in PK order.
    pub async fn fills_after(&self, c: Cursor, limit: i64) -> anyhow::Result<Vec<FillRow>> {
        let rows = sqlx::query_as::<_, FillRow>(
            "SELECT * FROM fill_events \
             WHERE (slot, txn_index, event_ordinal) > ($1, $2, $3) \
             ORDER BY slot, txn_index, event_ordinal LIMIT $4",
        )
        .bind(c.last_slot)
        .bind(c.last_txn_index)
        .bind(c.last_event_ordinal)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// All legs of one take (`(signature, txn_index)` group), for a full
    /// idempotent recompute.
    pub async fn legs_for(&self, signature: &str, txn_index: i64) -> anyhow::Result<Vec<FillRow>> {
        let rows = sqlx::query_as::<_, FillRow>(
            "SELECT * FROM fill_events WHERE signature = $1 AND txn_index = $2 \
             ORDER BY event_ordinal",
        )
        .bind(signature)
        .bind(txn_index)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn upsert_take(&self, t: &Take) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO takes (signature, txn_index, slot, block_time, market, taker, side, \
             leg_count, total_fill_base, total_fill_quote, total_taker_fee, avg_price) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) \
             ON CONFLICT (signature, txn_index) DO UPDATE SET \
             slot = EXCLUDED.slot, block_time = EXCLUDED.block_time, leg_count = EXCLUDED.leg_count, \
             total_fill_base = EXCLUDED.total_fill_base, total_fill_quote = EXCLUDED.total_fill_quote, \
             total_taker_fee = EXCLUDED.total_taker_fee, avg_price = EXCLUDED.avg_price",
        )
        .bind(&t.signature)
        .bind(t.txn_index)
        .bind(t.slot)
        .bind(t.block_time)
        .bind(&t.market)
        .bind(&t.taker)
        .bind(t.side)
        .bind(t.leg_count)
        .bind(t.total_fill_base)
        .bind(t.total_fill_quote)
        .bind(t.total_taker_fee)
        .bind(t.avg_price)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Recompute one market's rollup from its takes — idempotent.
    pub async fn recompute_market_stats(&self, market: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO market_stats (market, last_price, last_slot, take_count, volume_base, volume_quote) \
             SELECT t.market, \
               (SELECT avg_price FROM takes WHERE market = t.market ORDER BY slot DESC, txn_index DESC LIMIT 1), \
               COALESCE(MAX(t.slot), 0), COUNT(*), \
               COALESCE(SUM(t.total_fill_base), 0), COALESCE(SUM(t.total_fill_quote), 0) \
             FROM takes t WHERE t.market = $1 GROUP BY t.market \
             ON CONFLICT (market) DO UPDATE SET \
               last_price = EXCLUDED.last_price, last_slot = EXCLUDED.last_slot, \
               take_count = EXCLUDED.take_count, volume_base = EXCLUDED.volume_base, \
               volume_quote = EXCLUDED.volume_quote",
        )
        .bind(market)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    // ── /v1 reads ──────────────────────────────────────────────────────

    pub async fn recent_fills(
        &self,
        market: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<FillRow>> {
        let rows = sqlx::query_as::<_, FillRow>(
            "SELECT * FROM fill_events WHERE ($1::text IS NULL OR market = $1) \
             ORDER BY slot DESC, txn_index DESC, event_ordinal DESC LIMIT $2",
        )
        .bind(market)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_takes(&self, market: Option<&str>, limit: i64) -> anyhow::Result<Vec<Take>> {
        let rows = sqlx::query_as::<_, Take>(
            "SELECT * FROM takes WHERE ($1::text IS NULL OR market = $1) \
             ORDER BY slot DESC, txn_index DESC LIMIT $2",
        )
        .bind(market)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    pub async fn list_markets(&self) -> anyhow::Result<Vec<MarketStatsRow>> {
        let rows =
            sqlx::query_as::<_, MarketStatsRow>("SELECT * FROM market_stats ORDER BY market")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows)
    }

    pub async fn list_events(
        &self,
        kind: Option<&str>,
        market: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<EventEnvelope>> {
        let rows = sqlx::query_as::<_, EventEnvelope>(
            "SELECT slot, txn_index, signature, event_ordinal, block_time, kind, market, payload \
             FROM events WHERE ($1::text IS NULL OR kind = $1) \
             AND ($2::text IS NULL OR market = $2) \
             ORDER BY slot DESC, txn_index DESC, event_ordinal DESC LIMIT $3",
        )
        .bind(kind)
        .bind(market)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }
}

async fn write_fill(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    r: &FillRow,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        "INSERT INTO fill_events (slot, txn_index, signature, event_ordinal, block_time, market, \
         taker, leader, quote_authority, side, sector_idx, level_idx, fill_base, fill_quote, \
         fill_price, base_atoms_after, quote_atoms_after, nonce_after, taker_fee_atoms) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19) \
         ON CONFLICT DO NOTHING",
    )
    .bind(r.slot)
    .bind(r.txn_index)
    .bind(&r.signature)
    .bind(r.event_ordinal)
    .bind(r.block_time)
    .bind(&r.market)
    .bind(&r.taker)
    .bind(&r.leader)
    .bind(&r.quote_authority)
    .bind(r.side)
    .bind(r.sector_idx)
    .bind(r.level_idx)
    .bind(r.fill_base)
    .bind(r.fill_quote)
    .bind(r.fill_price)
    .bind(r.base_atoms_after)
    .bind(r.quote_atoms_after)
    .bind(r.nonce_after)
    .bind(r.taker_fee_atoms)
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected())
}

async fn write_envelope(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    coords: &EventCoords,
    event: &DropsetEvent,
) -> anyhow::Result<u64> {
    let res = sqlx::query(
        "INSERT INTO events (slot, txn_index, signature, event_ordinal, block_time, kind, market, payload) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING",
    )
    .bind(coords.slot)
    .bind(coords.txn_index)
    .bind(&coords.signature)
    .bind(coords.event_ordinal)
    .bind(coords.block_time)
    .bind(event.name())
    .bind(event_market(event))
    .bind(event_to_json(event))
    .execute(&mut **tx)
    .await?;
    Ok(res.rows_affected())
}
