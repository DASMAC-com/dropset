//! The Coinbase Exchange candles adapter (docs/data-feeds.md §9) — the CEX
//! reference price and the first, framework-proving source.
//!
//! It polls the public REST candles endpoint (keyless), which returns
//! `[time, low, high, open, close, volume]` arrays, newest-first, ≤ 300 per
//! request. The source **pages its own backfill**: the indexer's
//! take-newest-and-advance poll would skip the middle of a 60–90-day backlog,
//! so this walks `start → now` in ≤ `max_buckets` windows, reporting
//! `caught_up = false` until the present. Only **closed** buckets are emitted
//! — the currently-forming candle is excluded — so a store sink's
//! `ON CONFLICT DO NOTHING` never freezes an incomplete OHLCV row.
//!
//! The framework's paged-backfill helper (`feeds/src/backfill.rs`,
//! docs/data-feeds.md §13) is **deliberately not adopted here** — a settled
//! decision, not an open question. That helper exists to correct two failure
//! modes of a resume cursor used as an exclusive *lower* bound, and this
//! source has neither: its windows are bounded at **both** ends with the end
//! never reaching the present, it advances to the window it actually
//! requested rather than to the newest row it happened to see, and it commits
//! only after the await. Adopting `Backfill` here would add indirection and
//! remove nothing.
//!
//! The endpoint is keyed by a single product, so this adapter is deliberately
//! **not** a [`super::BatchQuotes`] venue: one source covers one product, and a
//! roster is several sources rather than one batched poll.

use crate::time::now_secs;
use crate::{Batch, Cursor, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Coinbase's per-request candle cap: a range wider than this many buckets is
/// rejected, so a backfill pages in windows no larger (docs/data-feeds.md §4).
/// It is the venue's constraint, so it lives with the venue — a collector
/// clamps its configured window to it rather than restating the number.
pub const MAX_CANDLES_PER_REQUEST: usize = 300;

/// A single closed OHLCV candle — the record this source yields. The pair,
/// exchange, and granularity live on the consumer's writer (they are constant
/// per feed), so a record carries only what varies bucket to bucket.
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
            max_buckets: max_buckets.clamp(1, MAX_CANDLES_PER_REQUEST),
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

/// A poll [`Source`] over one Coinbase product's **spot ticker** — the live
/// basis leg, as opposed to [`CoinbaseCandles`]' history.
///
/// The maker needs the current `<token>/USDC` print, not a closed bucket, so
/// this is a separate source over `/products/{id}/ticker` rather than a mode of
/// the candle feed: no cursor, no backfill, and each poll simply replaces the
/// last reading. Like the candles endpoint it is keyed by a single product, so
/// it is not a [`super::BatchQuotes`] venue either.
///
/// Which products a consumer subscribes to is the consumer's business — this
/// crate quotes whatever it is handed (docs/data-feeds.md §9).
pub struct CoinbaseTicker {
    http: HttpClient,
    name: String,
    product_id: String,
}

impl CoinbaseTicker {
    /// Build the source over `base_url` for one product (e.g. `EURC-USDC`).
    pub fn new(base_url: &str, product_id: impl Into<String>) -> Result<Self> {
        let product_id = product_id.into();
        Ok(Self {
            http: HttpClient::new(base_url)?,
            name: format!("coinbase:{product_id}"),
            product_id,
        })
    }

    /// Fetch this product's current price, or `None` when the venue answered
    /// without a usable one.
    pub async fn poll(&self) -> Result<Option<f64>> {
        let path = format!("/products/{}/ticker", self.product_id);
        let body: serde_json::Value = self.http.get_json(&path, &[]).await?;
        Ok(parse_coinbase_ticker(&body))
    }
}

#[async_trait]
impl Source for CoinbaseTicker {
    /// The product id and its price, so one consumer can drain several ticker
    /// sources into a single keyed cache.
    type Record = (String, f64);

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let price = self.poll().await?;
        Ok(Batch::new(ticker_records(&self.product_id, price)).with_caught_up(true))
    }
}

/// Map one poll's outcome onto this source's records. Split out from
/// [`CoinbaseTicker::next`] so the degrade path is testable without a network:
/// an unusable response must reach the sink as **no record**, never as a zero,
/// because a consumer caches by product id and an empty batch correctly leaves
/// the previous reading to age out.
fn ticker_records(product_id: &str, price: Option<f64>) -> Vec<(String, f64)> {
    price
        .map(|p| vec![(product_id.to_string(), p)])
        .unwrap_or_default()
}

/// Decode Coinbase's `{"price":"…","bid":"…","ask":"…"}` ticker response,
/// preferring the bid/ask mid and falling back to the last trade — the same
/// choice, for the same reason, as the Kraken adapter's
/// [`super::kraken::parse_kraken`].
pub fn parse_coinbase_ticker(body: &serde_json::Value) -> Option<f64> {
    let field = |k: &str| {
        body.get(k)
            .and_then(|v| match v {
                serde_json::Value::String(s) => s.parse::<f64>().ok(),
                other => other.as_f64(),
            })
            .filter(|v| v.is_finite() && *v > 0.0)
    };
    match (field("bid"), field("ask")) {
        (Some(bid), Some(ask)) => Some((bid + ask) / 2.0),
        _ => field("price"),
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

    #[test]
    fn resume_clamps_an_oversized_window_to_the_venue_cap() {
        // A caller asking for more buckets than Coinbase serves would get a
        // rejected request on every poll; the venue's own cap wins here.
        let source = CoinbaseCandles::resume(
            HttpClient::new("https://example.test").unwrap(),
            "cex:coinbase:EURC-USDC",
            "EURC-USDC",
            60,
            10_000,
            None,
            1_000,
        )
        .unwrap();
        assert_eq!(source.max_buckets, MAX_CANDLES_PER_REQUEST);
    }

    #[test]
    fn ticker_prefers_the_mid_over_the_last_trade() {
        // The mid (1.15300) must differ from the last trade (1.15290), or the
        // test passes just as happily against a `price`-only implementation
        // and asserts nothing about the preference it is named for.
        let body = serde_json::json!({
            "price": "1.15290", "bid": "1.15280", "ask": "1.15320", "volume": "3128704"
        });
        let got = parse_coinbase_ticker(&body).unwrap();
        assert!((got - 1.153_00).abs() < 1e-12, "got {got}");
    }

    #[test]
    fn ticker_falls_back_to_price_without_both_sides() {
        let body = serde_json::json!({ "price": "1.15290", "bid": "0" });
        assert!((parse_coinbase_ticker(&body).unwrap() - 1.152_90).abs() < 1e-12);
    }

    #[test]
    fn ticker_rejects_a_response_with_no_usable_price() {
        // Coinbase answers 400 for a product it has withdrawn, but an empty or
        // zero-priced body must not become a zero reading either.
        assert!(parse_coinbase_ticker(&serde_json::json!({})).is_none());
        assert!(parse_coinbase_ticker(&serde_json::json!({ "price": "0" })).is_none());
        assert!(parse_coinbase_ticker(&serde_json::json!({ "price": "n/a" })).is_none());
    }

    #[test]
    fn an_unusable_ticker_response_yields_no_record_rather_than_a_zero() {
        assert!(ticker_records("EURC-USDC", None).is_empty());
        assert_eq!(
            ticker_records("EURC-USDC", Some(1.1529)),
            vec![("EURC-USDC".to_string(), 1.1529)]
        );
    }

    #[test]
    fn ticker_source_is_named_per_product_so_several_do_not_collide_in_logs() {
        let source = CoinbaseTicker::new("https://example.test", "EURC-USDC").unwrap();
        assert_eq!(source.name(), "coinbase:EURC-USDC");
    }
}
