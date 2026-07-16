//! The Coinbase EURC/USDC reference feed (docs/fx-survey.md §4) — the survey's
//! CEX reference price and the gate's first, framework-proving source.
//!
//! It polls the Coinbase Exchange public REST candles endpoint (keyless), which
//! returns `[time, low, high, open, close, volume]` arrays, newest-first, ≤ 300
//! per request. The source **pages its own backfill**: the framework's
//! paged-backfill helper is still an open question (docs/data-feeds.md §7), and
//! the indexer's take-newest-and-advance poll would skip the middle of a
//! 60–90-day backlog, so this walks `start → now` in ≤ `max_buckets` windows,
//! reporting `caught_up = false` until the present. Only **closed** buckets are
//! emitted — the currently-forming candle is excluded — so the store sink's
//! `ON CONFLICT DO NOTHING` never freezes an incomplete OHLCV row.

use crate::config::now_secs;
use anyhow::Result;
use async_trait::async_trait;
use dropset_feeds::{Batch, Cursor, HttpClient, Source};
use serde::{Deserialize, Serialize};

/// A single closed OHLCV candle — the record this source yields. The pair,
/// exchange, and granularity live on the writer (they are constant per feed),
/// so a record carries only what varies bucket to bucket.
#[derive(Clone, Debug, PartialEq)]
pub struct Candle {
    /// Epoch-second bucket open.
    pub bucket_start: i64,
    pub low: f64,
    pub high: f64,
    pub open: f64,
    pub close: f64,
    pub volume: f64,
}

/// The Coinbase Exchange candle tuple, decoded positionally:
/// `[time, low, high, open, close, volume]`.
type CandleTuple = (i64, f64, f64, f64, f64, f64);

/// This source's opaque resume position: the next epoch second still to fetch.
#[derive(Serialize, Deserialize)]
struct CexCursor {
    next_start: i64,
}

/// A poll [`Source`] over one Coinbase product's candles.
pub struct CoinbaseCandles {
    http: HttpClient,
    name: String,
    product_id: String,
    granularity: i64,
    max_buckets: usize,
    /// The oldest epoch second not yet persisted; advances as windows drain.
    next_start: i64,
}

impl CoinbaseCandles {
    /// Build the source, resuming from a saved framework cursor when present
    /// (a poll source resumes from its cursor, docs/data-feeds.md §3) and
    /// otherwise starting the backfill at `default_start`.
    pub fn resume(
        http: HttpClient,
        name: impl Into<String>,
        product_id: impl Into<String>,
        granularity: i64,
        max_buckets: usize,
        resume: Option<Cursor>,
        default_start: i64,
    ) -> Result<Self> {
        let next_start = match resume {
            Some(cursor) => cursor.get::<CexCursor>()?.next_start,
            None => default_start,
        };
        Ok(Self {
            http,
            name: name.into(),
            product_id: product_id.into(),
            granularity: granularity.max(1),
            max_buckets: max_buckets.max(1),
            next_start,
        })
    }

    /// The start of the currently-forming bucket: everything strictly before it
    /// is closed and immutable.
    fn closed_boundary(&self) -> i64 {
        let now = now_secs();
        now - now.rem_euclid(self.granularity)
    }
}

#[async_trait]
impl Source for CoinbaseCandles {
    type Record = Candle;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let closed_boundary = self.closed_boundary();
        // Nothing has closed since the last window — report caught up and let
        // the runner sleep. No cursor change: the position is already saved.
        if self.next_start >= closed_boundary {
            return Ok(Batch::new(vec![]).with_caught_up(true));
        }

        let end = window_end(
            self.next_start,
            self.granularity,
            self.max_buckets,
            closed_boundary,
        );
        let granularity_s = self.granularity.to_string();
        let start_s = self.next_start.to_string();
        let end_s = end.to_string();
        let path = format!("/products/{}/candles", self.product_id);
        let raw: Vec<CandleTuple> = self
            .http
            .get_json(
                &path,
                &[
                    ("granularity", granularity_s.as_str()),
                    ("start", start_s.as_str()),
                    ("end", end_s.as_str()),
                ],
            )
            .await?;

        let records = assemble(raw, self.next_start, closed_boundary);
        // Advance past the whole window we requested, not just the last row:
        // an empty window (a gap with no trades) must still move the cursor or
        // the backfill stalls on it forever.
        self.next_start = end;
        let caught_up = end >= closed_boundary;
        let cursor = Cursor::new(&CexCursor {
            next_start: self.next_start,
        })?;
        Ok(Batch::new(records)
            .with_cursor(cursor)
            .with_caught_up(caught_up))
    }
}

/// The end of the next backfill window: at most `max_buckets` past `next_start`,
/// clamped to the last closed boundary so a request never spans more than
/// Coinbase's per-request cap and never reaches into the forming bucket.
fn window_end(next_start: i64, granularity: i64, max_buckets: usize, closed_boundary: i64) -> i64 {
    let span = granularity * max_buckets as i64;
    (next_start + span).min(closed_boundary)
}

/// Turn a raw Coinbase response (newest-first, possibly including the forming
/// bucket at the window end) into the batch's records: keep only closed buckets
/// at or after `next_start`, and order oldest-first (the store sink expects
/// ascending records).
fn assemble(raw: Vec<CandleTuple>, next_start: i64, closed_boundary: i64) -> Vec<Candle> {
    let mut records: Vec<Candle> = raw
        .into_iter()
        .filter(|(t, ..)| *t >= next_start && *t < closed_boundary)
        .map(|(t, low, high, open, close, volume)| Candle {
            bucket_start: t,
            low,
            high,
            open,
            close,
            volume,
        })
        .collect();
    records.sort_by_key(|c| c.bucket_start);
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_caps_at_the_bucket_budget_mid_backfill() {
        // Far from the present: the window is exactly `max_buckets` wide.
        let end = window_end(1_000, 60, 300, 10_000_000);
        assert_eq!(end, 1_000 + 60 * 300);
    }

    #[test]
    fn window_clamps_to_the_closed_boundary_near_the_present() {
        // Close to the present: the window stops at the last closed bucket, so
        // the forming candle is never requested.
        let end = window_end(1_000, 60, 300, 1_600);
        assert_eq!(end, 1_600);
    }

    #[test]
    fn assemble_drops_the_forming_bucket_and_sorts_ascending() {
        // Newest-first, with the forming bucket (t == closed_boundary) present.
        let closed_boundary = 240;
        let raw = vec![
            (240, 1.0, 1.0, 1.0, 1.0, 1.0), // forming — dropped
            (180, 1.2, 1.3, 1.25, 1.28, 5.0),
            (120, 1.1, 1.2, 1.15, 1.18, 4.0),
            (60, 1.0, 1.1, 1.05, 1.08, 3.0),
        ];
        let got = assemble(raw, 60, closed_boundary);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(times, vec![60, 120, 180]);
    }

    #[test]
    fn assemble_drops_buckets_before_the_resume_point() {
        // A defensive filter: nothing before `next_start` leaks into the batch.
        let raw = vec![
            (180, 1.2, 1.3, 1.25, 1.28, 5.0),
            (120, 1.1, 1.2, 1.15, 1.18, 4.0),
            (60, 1.0, 1.1, 1.05, 1.08, 3.0),
        ];
        let got = assemble(raw, 120, 10_000);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(times, vec![120, 180]);
    }
}
