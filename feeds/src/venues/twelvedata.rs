//! The Twelve Data time-series adapter (docs/data-feeds.md §9) — an
//! independent FX cross-check against the OANDA anchor.
//!
//! It polls `/time_series` for one symbol, authenticated by an `apikey` query
//! parameter supplied by the caller. Three properties of this venue shape the
//! adapter, and each was measured against a live key rather than read off the
//! docs:
//!
//! * **It defaults to exchange-local time, not UTC.** A default request for
//!   AUD/USD returned `2026-08-14 10:26` when the actual UTC time was `00:26` —
//!   Sydney, the pair's home exchange. Every request here therefore sends
//!   `timezone=UTC` explicitly. Without it the stored `bucket_start` would be
//!   ten hours off and look entirely plausible.
//! * **It publishes no volume.** A bar is `{datetime, open, high, low, close}`
//!   and nothing else, so [`Candle::volume`] is written `0.0`. A consumer must
//!   not read volume on a row from this source as meaningful — see the note on
//!   [`Candle`] about volume being comparable only within a source.
//! * **It publishes a complete bar grid every day, including weekends**, where
//!   OANDA publishes none at all. That is a coverage difference between two
//!   vendors, not evidence about either: a measured Saturday range of 3.68 bps
//!   sits in the same band as a genuinely traded 24/7 crypto tape's 6.92 bps.
//!   The rows are ingested as-is; prefer the zero-bar source when the question
//!   is *whether a session existed*, and never pool the two into one volatility
//!   figure.
//!
//! An error is reported **in a 200 body** as often as by status code, so the
//! response is decoded through an envelope that checks for it rather than
//! relying on the transport's `error_for_status`.

use super::Candle;
use crate::time::{format_civil_utc, now_secs, parse_civil_utc};
use crate::{Batch, Cursor, HttpClient, Source};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Twelve Data's per-request bar cap, verified against the venue: a
/// 5000-minute window returns exactly 5000 bars. The adapter advances its
/// cursor past the whole requested window, so a window wider than the cap
/// would drop every bar past it and never ask again — this clamp is what
/// keeps that unreachable.
pub const MAX_BARS_PER_REQUEST: usize = 5000;

/// The canonical name of this venue's credential ([`crate::secrets`]). The
/// provider segment is one word — `twelve-data` would fold to the same
/// environment variable as a `twelve/data` name.
pub const SECRET_NAME: &str = "twelvedata/api-key";

/// The floor between two requests on this venue.
///
/// The free tier allows **8 requests per minute**, which is far stricter than
/// the shared client's 250 ms default — and the default is what would apply
/// during a **backfill**, where the runner loops without pausing (its
/// `poll_interval` only governs the caught-up state). At 250 ms that is ~240
/// requests a minute, 30× the tier, so a cold backfill would be rate-limited
/// immediately and burn the 800/day budget getting nowhere.
///
/// 8 seconds is 7.5 requests a minute — just inside the limit, and cheap: a
/// 60-day minute-bar backfill is ~18 requests at 5000 bars each, so the floor
/// costs about two minutes once, not per poll.
///
/// **This floor holds the venue's rate; it does not hold the daily budget.**
/// Twelve Data publishes both — 8 requests a minute *and* 800 credits a day —
/// and only the first is a rate an interval can bound. The daily budget is held
/// by the collector's steady-state cadence instead (300 s, ≈288 requests/day),
/// and that holds only per continuous run. Unlike Alpha Vantage, whose floor is
/// standing in for a quota outright, this constant is doing the job it can
/// actually do; the budget is simply someone else's. See
/// [`crate::HttpClient::with_min_interval`] for why the distinction matters.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(8);

/// This source's opaque resume position: the next epoch second still to fetch.
#[derive(Serialize, Deserialize)]
struct FxCursor {
    next_start: i64,
}

/// One bar. Every field is a string, including the numbers.
#[derive(Debug, Deserialize)]
struct RawBar {
    /// Civil time, `YYYY-MM-DD HH:MM:SS`, in whatever zone the request asked
    /// for — which this adapter always pins to UTC.
    datetime: String,
    open: String,
    high: String,
    low: String,
    close: String,
}

/// The response envelope. Twelve Data answers an error as a 200 carrying
/// `{"status":"error","code":429,"message":"…"}`, so both arms are modelled and
/// [`check_response`] decides which one arrived.
#[derive(Debug, Deserialize)]
struct TimeSeriesResponse {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    values: Vec<RawBar>,
}

/// A poll [`Source`] over one Twelve Data symbol's bars.
pub struct TwelveDataCandles {
    http: HttpClient,
    name: String,
    /// The venue's own symbol (`AUD/USD`), which is not the canonical
    /// `product_id` a collector stores.
    symbol: String,
    /// The venue's interval token (`1min`), derived once at construction.
    interval: &'static str,
    granularity_secs: i64,
    max_bars: usize,
    /// The oldest epoch second not yet persisted; advances as windows drain.
    next_start: i64,
}

impl TwelveDataCandles {
    /// Build the source, resuming from a saved framework cursor when present
    /// and otherwise starting the backfill at `default_start`.
    ///
    /// `api_key` is injected rather than read from the environment here
    /// (docs/data-feeds.md §4). It is handed straight to the transport as a
    /// marked credential parameter rather than kept on this struct, so the
    /// adapter never spells the key into a per-request query itself — that is
    /// what lets the transport redact it out of an error URL.
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        base_url: &str,
        api_key: &str,
        name: impl Into<String>,
        symbol: impl Into<String>,
        granularity_secs: i64,
        max_bars: usize,
        resume: Option<Cursor>,
        default_start: i64,
    ) -> Result<Self> {
        let next_start = match resume {
            Some(cursor) => cursor.get::<FxCursor>()?.next_start,
            None => default_start,
        };
        Ok(Self {
            http: HttpClient::new(base_url)?
                .with_min_interval(MIN_REQUEST_INTERVAL)
                .with_secret_query_param("apikey", api_key),
            name: name.into(),
            symbol: symbol.into(),
            interval: interval_token(granularity_secs)?,
            granularity_secs,
            max_bars: max_bars.clamp(1, MAX_BARS_PER_REQUEST),
            next_start,
        })
    }

    /// The start of the currently-forming bucket: everything strictly before it
    /// is closed. Twelve Data carries no per-bar "complete" marker, so unlike
    /// the OANDA adapter this boundary is the *only* thing keeping a forming
    /// bar out of the store.
    fn closed_boundary(&self) -> i64 {
        let now = now_secs();
        now - now.rem_euclid(self.granularity_secs)
    }
}

#[async_trait]
impl Source for TwelveDataCandles {
    type Record = Candle;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let closed_boundary = self.closed_boundary();
        if self.next_start >= closed_boundary {
            return Ok(Batch::new(vec![]).with_caught_up(true));
        }

        let end = window_end(
            self.next_start,
            self.granularity_secs,
            self.max_bars,
            closed_boundary,
        );
        let start_s = format_civil_utc(self.next_start);
        // The venue treats `end_date` as inclusive, so step back one bucket to
        // keep the window half-open like every other source's here.
        let end_s = format_civil_utc(end - self.granularity_secs);
        let body: TimeSeriesResponse = self
            .http
            .get_json(
                "/time_series",
                &[
                    ("symbol", self.symbol.as_str()),
                    ("interval", self.interval),
                    // Never omit: the venue defaults to exchange-local time.
                    ("timezone", "UTC"),
                    ("start_date", start_s.as_str()),
                    ("end_date", end_s.as_str()),
                    // `apikey` is appended by the transport, which holds it as a
                    // marked credential.
                ],
            )
            .await?;

        let values = check_response(body)?;
        let records = assemble(values, self.next_start, end);
        // Advance past the whole requested window, not to the newest row.
        self.next_start = end;
        let caught_up = end >= closed_boundary;
        let cursor = Cursor::new(&FxCursor {
            next_start: self.next_start,
        })?;
        Ok(Batch::new(records)
            .with_cursor(cursor)
            .with_caught_up(caught_up))
    }
}

/// Map a bucket width in seconds onto the venue's interval token.
///
/// An explicit allowlist for the same reason the OANDA adapter has one: only
/// these widths exist, and a computed string would defer the failure to a
/// response body nobody reads.
fn interval_token(secs: i64) -> Result<&'static str> {
    Ok(match secs {
        60 => "1min",
        300 => "5min",
        900 => "15min",
        1_800 => "30min",
        2_700 => "45min",
        3_600 => "1h",
        7_200 => "2h",
        14_400 => "4h",
        86_400 => "1day",
        604_800 => "1week",
        other => {
            return Err(anyhow!(
                "Twelve Data has no interval of {other}s; supported widths are \
                 1/5/15/30/45m, 1/2/4h, 1day, 1week"
            ))
        }
    })
}

/// Unwrap the envelope, turning an error reported inside a 200 body into a
/// real error. A rate-limit refusal arrives this way, and treating it as an
/// empty result would silently advance the cursor past a window that was never
/// fetched — the one failure mode that loses data permanently.
fn check_response(body: TimeSeriesResponse) -> Result<Vec<RawBar>> {
    if body.status.as_deref() == Some("error") || body.code.is_some() {
        let code = body.code.unwrap_or_default();
        let message = body.message.unwrap_or_else(|| "no message".to_string());
        return Err(anyhow!("Twelve Data refused the request: {code} {message}"));
    }
    Ok(body.values)
}

/// The end of the next backfill window.
fn window_end(
    next_start: i64,
    granularity_secs: i64,
    max_bars: usize,
    closed_boundary: i64,
) -> i64 {
    let span = granularity_secs * max_bars as i64;
    (next_start + span).min(closed_boundary)
}

/// Turn a raw response (newest-first) into the batch's records: keep bars
/// inside `[next_start, end)`, oldest-first. An undecodable bar is dropped
/// rather than failing the batch, for the same reason as the OANDA adapter.
fn assemble(raw: Vec<RawBar>, next_start: i64, end: i64) -> Vec<Candle> {
    let mut records: Vec<Candle> = raw
        .into_iter()
        .filter_map(|bar| decode(&bar).ok())
        .filter(|c| c.bucket_start >= next_start && c.bucket_start < end)
        .collect();
    records.sort_by_key(|c| c.bucket_start);
    records
}

/// Decode one bar into the shared [`Candle`] record.
fn decode(raw: &RawBar) -> Result<Candle> {
    let price = |field: &str, value: &str| -> Result<f64> {
        value
            .parse::<f64>()
            .with_context(|| format!("Twelve Data {field} price {value:?} is not a number"))
    };
    Ok(Candle {
        bucket_start: parse_civil_utc(&raw.datetime)?,
        low: price("low", &raw.low)?,
        high: price("high", &raw.high)?,
        open: price("open", &raw.open)?,
        close: price("close", &raw.close)?,
        // The venue publishes none; see the module note.
        volume: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::civil_to_epoch_secs;
    use crate::venues::requests_per_window;

    #[test]
    fn the_floor_stays_inside_the_free_tiers_eight_per_minute() {
        let per_minute = requests_per_window(MIN_REQUEST_INTERVAL, Duration::from_secs(60));
        assert!(
            per_minute < 8.0,
            "{per_minute} requests/minute does not sit strictly inside Twelve \
             Data's documented 8/min free tier"
        );
    }

    /// A captured response: two AUD/USD 1min bars, newest-first, as returned
    /// with `timezone=UTC`.
    fn captured_response() -> TimeSeriesResponse {
        serde_json::from_value(serde_json::json!({
            "meta": {
                "symbol": "AUD/USD", "interval": "1min",
                "currency_base": "Australian Dollar",
                "currency_quote": "US Dollar", "type": "Physical Currency"
            },
            "values": [
                { "datetime": "2026-08-14 00:26:00", "open": "0.70648",
                  "high": "0.70648", "low": "0.70644", "close": "0.70645" },
                { "datetime": "2026-08-14 00:25:00", "open": "0.70644",
                  "high": "0.70651", "low": "0.70644", "close": "0.7065" }
            ],
            "status": "ok"
        }))
        .expect("captured response decodes")
    }

    #[test]
    fn decodes_a_captured_bar_with_zero_volume() {
        let body = captured_response();
        let got = decode(&body.values[0]).unwrap();
        assert_eq!(
            got,
            Candle {
                bucket_start: civil_to_epoch_secs(2026, 8, 14, 0, 26, 0),
                low: 0.706_44,
                high: 0.706_48,
                open: 0.706_48,
                close: 0.706_45,
                volume: 0.0,
            }
        );
    }

    #[test]
    fn assemble_orders_oldest_first() {
        // The venue returns newest-first; the store sink expects ascending.
        let body = captured_response();
        let start = civil_to_epoch_secs(2026, 8, 14, 0, 0, 0);
        let end = civil_to_epoch_secs(2026, 8, 14, 1, 0, 0);
        let got = assemble(body.values, start, end);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(
            times,
            vec![
                civil_to_epoch_secs(2026, 8, 14, 0, 25, 0),
                civil_to_epoch_secs(2026, 8, 14, 0, 26, 0),
            ]
        );
    }

    #[test]
    fn assemble_excludes_bars_outside_the_requested_window() {
        let body = captured_response();
        let start = civil_to_epoch_secs(2026, 8, 14, 0, 27, 0);
        let end = civil_to_epoch_secs(2026, 8, 14, 1, 0, 0);
        assert!(assemble(body.values, start, end).is_empty());
    }

    #[test]
    fn an_error_reported_inside_a_200_body_is_a_real_error() {
        // The failure that matters: a rate-limit refusal must not read as an
        // empty window, which would advance the cursor past unfetched data.
        let body: TimeSeriesResponse = serde_json::from_value(serde_json::json!({
            "code": 429,
            "message": "You have run out of API credits for the current minute.",
            "status": "error"
        }))
        .unwrap();
        let err = check_response(body).unwrap_err().to_string();
        assert!(err.contains("429"), "{err}");
    }

    #[test]
    fn a_healthy_response_passes_the_envelope_check() {
        assert_eq!(check_response(captured_response()).unwrap().len(), 2);
    }

    #[test]
    fn an_unsupported_interval_is_rejected_at_construction() {
        assert!(TwelveDataCandles::resume(
            "https://example.test",
            "key",
            "fx:twelvedata:AUD-USD",
            "AUD/USD",
            180,
            500,
            None,
            1_000,
        )
        .is_err());
    }

    #[test]
    fn resume_clamps_an_oversized_window_to_the_venue_cap() {
        let source = TwelveDataCandles::resume(
            "https://example.test",
            "key",
            "fx:twelvedata:AUD-USD",
            "AUD/USD",
            60,
            10_000,
            None,
            1_000,
        )
        .unwrap();
        assert_eq!(source.max_bars, MAX_BARS_PER_REQUEST);
    }
}
