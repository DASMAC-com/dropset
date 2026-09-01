//! Registering this collector's roster in the instruments dimension.
//!
//! The exact mirror of [`crate::pyth_roster`]: that module *reads* roster
//! reference data the store owns, and this one *writes* the roster the
//! environment owns into the store. Both run once, at startup, for the same
//! reason — a process whose effective roster is fixed at start can state it in
//! one log line.
//!
//! **Why the collectors write this at all.** The dimension has to answer "what
//! instrument is `EURC-USDC`" for a dashboard, and the alternative was deriving
//! the product set from the measurement tables with `SELECT DISTINCT
//! product_id`. That is a full scan of an unbounded table: `cex_prices` and
//! `spot_ticks` both key on `(source, product_id, …)`, so `product_id` leads no
//! index, and both tables deliberately carry no secondary index because their
//! dominant read is a time-ordered scan of one series. Paying write overhead on
//! the two hottest tables in the store to answer a question about a set that
//! changes a few times a year is the wrong trade, so the collectors state the
//! set instead — which they already know, for free, before they connect.
//!
//! **This records the configured roster, not the observed data.** A product
//! appears here as soon as a collector that polls it starts, whether or not the
//! venue has ever answered. That is the useful direction for a dimension: a
//! configured pair with no data is a fact worth surfacing, and it is what the
//! ingestion dashboard's coverage guarantee is about. Nothing here is a
//! freshness signal — see the note on [`register`].
//!
//! `dropset-migrate` owns both tables (`0009_instruments.sql`), and the
//! currency-kind reference data is seeded there; nothing here writes it.

use anyhow::{Context, Result};
use dropset_feeds::now_secs;
use sqlx::PgPool;

/// Register every canonical product id in this collector's roster.
///
/// Idempotent: the upsert keeps `first_registered_at` from the first
/// registration and moves `last_registered_at` every time, so a restart costs
/// one statement and changes nothing else. Several collectors polling the same
/// pair — `EUR-USD` is on OANDA, Twelve Data, Alpha Vantage and Pyth —
/// converge on one row, because the dimension is per-product rather than per
/// `(source, product)`: "what instrument is this" has the same answer whoever
/// measured it.
///
/// **Neither timestamp is a data-freshness signal, and no caller may read one
/// as one.** Both track process starts: a collector whose venue has answered
/// nothing for a week still refreshes `last_registered_at` on every restart.
/// What they answer together is "is this pair still in somebody's roster, or is
/// its row a leftover from a roster it was dropped from" — nothing deletes a
/// row when a pair leaves a roster, so without this the registry could not
/// distinguish the two.
///
/// **A failure here fails startup**, like the schema assertion it runs beside.
/// Postgres is a hard dependency for a collector — it is the thing being
/// written to — so a pool that cannot take this one small statement is not
/// going to take the measurements either, and failing at startup names the
/// problem far better than a dimension that is quietly missing a pair.
/// (Contrast the maker bot, where Postgres is deliberately a *soft* dependency
/// and an unreachable store degrades quoting rather than preventing a start.)
pub async fn register(pool: &PgPool, product_ids: &[String]) -> Result<()> {
    // An empty roster never reaches here — `parse_roster` rejects one, because a
    // collector with nothing to poll looks perfectly healthy while writing
    // nothing. Guarded anyway so a future caller assembling ids some other way
    // gets a no-op rather than a statement that expands an empty array.
    if product_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(include_str!("../queries/instrument_register.sql"))
        .bind(product_ids)
        .bind(now_secs())
        .execute(pool)
        .await
        .with_context(|| {
            format!(
                "registering {} product(s) in the instruments dimension \
                 (`instrument_registry`); a database predating \
                 `0009_instruments.sql` is the likely cause",
                product_ids.len()
            )
        })?;
    Ok(())
}
