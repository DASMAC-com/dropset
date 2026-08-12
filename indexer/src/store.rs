//! Postgres persistence: idempotent raw-event writes keyed on the event
//! PK, the aggregator's watermark + reads, and the `/v1` read queries.

use crate::decode::{decode_tx, RawTx};
use crate::model::{event_market, event_to_json, FillRow, MarketStatsRow, Take};
use crate::model::{DecodedEvent, EventCoords};
use async_trait::async_trait;
use dropset_feeds::StoreWriter;
use dropset_sdk::events::DropsetEvent;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

/// The aggregator watermark: the last event coordinate folded. Carries
/// `last_signature` so the tuple is unique (the RPC path pins `txn_index`
/// to 0, collapsing `(slot, txn_index, event_ordinal)` across takes).
#[derive(Clone, Debug, Default, sqlx::FromRow)]
pub struct Cursor {
    pub last_slot: i64,
    pub last_txn_index: i64,
    pub last_event_ordinal: i64,
    pub last_signature: String,
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
    /// Connect, and assert the shared schema this build expects is already
    /// present.
    ///
    /// The indexer does **not** create its own tables: `dropset-db-schema` is
    /// the single schema owner and `dropset-migrate` is the only thing that
    /// issues DDL (docs/data-feeds.md §8). The indexer is DB-primary — it has
    /// nothing to do without its tables — so a database that has not been
    /// migrated is a startup failure here, with an error naming the fix,
    /// rather than a bare `relation … does not exist` on the first write.
    pub async fn connect(url: &str) -> anyhow::Result<Self> {
        let pool = PgPoolOptions::new().max_connections(8).connect(url).await?;
        dropset_db_schema::require_schema(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn cursor(&self) -> anyhow::Result<Cursor> {
        let c = sqlx::query_as::<_, Cursor>(include_str!("../queries/cursor_get.sql"))
            .fetch_one(&self.pool)
            .await?;
        Ok(c)
    }

    pub async fn set_cursor(&self, c: &Cursor) -> anyhow::Result<()> {
        sqlx::query(include_str!("../queries/cursor_set.sql"))
            .bind(c.last_slot)
            .bind(c.last_txn_index)
            .bind(c.last_event_ordinal)
            .bind(&c.last_signature)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Fill legs with coordinates strictly after the cursor, in PK order.
    /// `signature` is part of the compare/order tuple so the strict `>`
    /// never skips a leg when two takes share `(slot, txn_index,
    /// event_ordinal)` (the RPC path pins `txn_index` to 0).
    pub async fn fills_after(&self, c: &Cursor, limit: i64) -> anyhow::Result<Vec<FillRow>> {
        let rows = sqlx::query_as::<_, FillRow>(include_str!("../queries/fills_after.sql"))
            .bind(c.last_slot)
            .bind(c.last_txn_index)
            .bind(c.last_event_ordinal)
            .bind(&c.last_signature)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    /// All legs of one take (`(signature, txn_index)` group), for a full
    /// idempotent recompute.
    pub async fn legs_for(&self, signature: &str, txn_index: i64) -> anyhow::Result<Vec<FillRow>> {
        let rows = sqlx::query_as::<_, FillRow>(include_str!("../queries/legs_for.sql"))
            .bind(signature)
            .bind(txn_index)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn upsert_take(&self, t: &Take) -> anyhow::Result<()> {
        sqlx::query(include_str!("../queries/take_upsert.sql"))
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
        sqlx::query(include_str!("../queries/market_stats_recompute.sql"))
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
        let rows = sqlx::query_as::<_, FillRow>(include_str!("../queries/recent_fills.sql"))
            .bind(market)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn list_takes(&self, market: Option<&str>, limit: i64) -> anyhow::Result<Vec<Take>> {
        let rows = sqlx::query_as::<_, Take>(include_str!("../queries/takes_list.sql"))
            .bind(market)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }

    pub async fn list_markets(&self) -> anyhow::Result<Vec<MarketStatsRow>> {
        let rows = sqlx::query_as::<_, MarketStatsRow>(include_str!("../queries/markets_list.sql"))
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
        let rows = sqlx::query_as::<_, EventEnvelope>(include_str!("../queries/events_list.sql"))
            .bind(kind)
            .bind(market)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows)
    }
}

/// The indexer's half of the framework store sink: decode each fetched
/// transaction and write its events as rows.
///
/// The framework owns the batch transaction and the resume cursor
/// (docs/data-feeds.md §3); this owns the record → table mapping, and writes
/// idempotently on the event PK so a re-delivered batch is absorbed rather
/// than duplicated. Decode happens here rather than in a separate stage
/// because a transaction carrying no Dropset events should cost no rows and
/// no extra pass.
pub struct EventWriter;

#[async_trait]
impl StoreWriter for EventWriter {
    type Record = RawTx;

    async fn write_batch(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        records: &[RawTx],
    ) -> anyhow::Result<u64> {
        let mut written = 0u64;
        for raw in records {
            for de in decode_tx(raw) {
                written += write_event(tx, &de).await?;
            }
        }
        Ok(written)
    }
}

/// Write one decoded event: fills go to the typed table, everything else to
/// the JSONB fidelity table. Returns rows actually written, which is `0` for
/// an event already stored.
async fn write_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    de: &DecodedEvent,
) -> anyhow::Result<u64> {
    match &de.event {
        DropsetEvent::Fill(f) => write_fill(tx, &FillRow::from_event(&de.coords, f)).await,
        other => write_envelope(tx, &de.coords, other).await,
    }
}

async fn write_fill(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    r: &FillRow,
) -> anyhow::Result<u64> {
    let res = sqlx::query(include_str!("../queries/fill_insert.sql"))
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
    let res = sqlx::query(include_str!("../queries/event_insert.sql"))
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
