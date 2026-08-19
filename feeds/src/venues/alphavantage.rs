// cspell:word outputsize
//! The Alpha Vantage FX daily adapter (docs/data-feeds.md §9) — a **daily-only**
//! third opinion on the FX rate.
//!
//! This adapter is deliberately narrow, because the free tier is:
//!
//! * **`FX_INTRADAY` is premium-gated.** Verified against a live free key, which
//!   answers `"This is a premium endpoint."` So there is no minute bar to be had
//!   here at any polling cadence, and this source cannot substitute for the
//!   OANDA anchor — it corroborates the daily close and nothing more.
//! * **The budget is 25 requests per day**, which is the whole account, not the
//!   endpoint. A collector must poll on the order of hours, never minutes.
//! * **No volume is published**, so [`Candle::volume`] is written `0.0`.
//!
//! `FX_DAILY` takes no date-range parameter — it returns the venue's whole
//! published series — so this source's window discipline differs from every
//! other one here. It always asks for `outputsize=full`, which is free and
//! returned 5000 daily bars reaching back to 2007 when measured. That depth is
//! what makes the cursor safe: a response always covers the resume point, so
//! there is no window between "oldest row returned" and "where we left off"
//! that could be skipped unnoticed.

use super::Candle;
use crate::time::{now_secs, parse_civil_utc};
use crate::{Batch, Cursor, HttpClient, Source};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// The only bucket width this source can serve.
pub const GRANULARITY_SECS: i64 = 86_400;

/// The canonical name of this venue's credential ([`crate::secrets`]).
pub const SECRET_NAME: &str = "alphavantage/api-key";

/// The floor between two requests on this venue.
///
/// The free tier allows **25 requests per day for the whole account**, so an
/// hour between requests caps this source at 24 — inside the budget with a
/// request to spare, and far below the six-hour poll a collector actually
/// runs at.
///
/// It is therefore not load-bearing today: this source fetches the entire
/// published series in one call and never pages, so the floor never binds.
/// It is here to encode the constraint at the transport, where it will bind
/// if anyone later adds paging or a retry loop — the shared client's 250 ms
/// default would exhaust a 25/day budget in seven seconds.
///
/// **This interval approximates a quota, and cannot enforce one — do not read
/// the arithmetic above as a budget guarantee.** "24 requests a day" holds only
/// across a single continuous run: the gate is in-process state that resets when
/// the process does, so a crash-loop, or a few local stack cycles in one
/// afternoon, spends the account's 25 while every individual pacing decision
/// stays correct. The exposure is invisible for exactly that reason — the
/// steady-state arithmetic checks out. Closing it needs durable per-day state,
/// which is not built; see [`crate::HttpClient::with_min_interval`] for the
/// canonical statement of the rate-versus-quota distinction.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(3_600);

/// This source's opaque resume position: the next epoch second still to fetch.
#[derive(Serialize, Deserialize)]
struct FxCursor {
    next_start: i64,
}

/// One daily bar. The field names carry the venue's ordinal prefixes.
#[derive(Debug, Deserialize)]
struct RawBar {
    #[serde(rename = "1. open")]
    open: String,
    #[serde(rename = "2. high")]
    high: String,
    #[serde(rename = "3. low")]
    low: String,
    #[serde(rename = "4. close")]
    close: String,
}

/// The response envelope.
///
/// A refusal — premium gate, exhausted budget, bad symbol — arrives as a **200**
/// carrying one of `Note` / `Information` / `Error Message` and no series at
/// all, so all three are modelled. `Meta Data` also contains a nested
/// `"1. Information"`, which is why the gate keys are matched at the top level
/// only.
#[derive(Debug, Deserialize)]
struct FxDailyResponse {
    #[serde(rename = "Note")]
    note: Option<String>,
    #[serde(rename = "Information")]
    information: Option<String>,
    #[serde(rename = "Error Message")]
    error_message: Option<String>,
    /// Keyed by `YYYY-MM-DD`; a `BTreeMap` so iteration is date-ordered.
    #[serde(rename = "Time Series FX (Daily)")]
    series: Option<BTreeMap<String, RawBar>>,
}

/// A poll [`Source`] over one currency pair's daily FX bars.
pub struct AlphaVantageDaily {
    http: HttpClient,
    name: String,
    /// The venue takes the pair as two separate parameters rather than one
    /// symbol — a third distinct spelling among the FX vendors, and the reason
    /// the stored `product_id` is canonical rather than venue-native.
    from_symbol: String,
    to_symbol: String,
    api_key: String,
    /// The oldest epoch second not yet persisted; advances as the series drains.
    next_start: i64,
}

impl AlphaVantageDaily {
    /// Build the source, resuming from a saved framework cursor when present
    /// and otherwise starting at `default_start`.
    ///
    /// `api_key` is injected rather than read from the environment here
    /// (docs/data-feeds.md §4).
    pub fn resume(
        base_url: &str,
        api_key: &str,
        name: impl Into<String>,
        from_symbol: impl Into<String>,
        to_symbol: impl Into<String>,
        resume: Option<Cursor>,
        default_start: i64,
    ) -> Result<Self> {
        let next_start = match resume {
            Some(cursor) => cursor.get::<FxCursor>()?.next_start,
            None => default_start,
        };
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            name: name.into(),
            from_symbol: from_symbol.into(),
            to_symbol: to_symbol.into(),
            api_key: api_key.to_string(),
            next_start,
        })
    }

    /// The start of today's forming bar: every dated bar strictly before it is
    /// closed. The venue publishes the current day's partial bar under today's
    /// date, and this is what keeps it out of the store.
    fn closed_boundary(&self) -> i64 {
        let now = now_secs();
        now - now.rem_euclid(GRANULARITY_SECS)
    }
}

#[async_trait]
impl Source for AlphaVantageDaily {
    type Record = Candle;

    fn name(&self) -> &str {
        &self.name
    }

    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        let closed_boundary = self.closed_boundary();
        if self.next_start >= closed_boundary {
            return Ok(Batch::new(vec![]).with_caught_up(true));
        }

        let body: FxDailyResponse = self
            .http
            .get_json(
                "/query",
                &[
                    ("function", "FX_DAILY"),
                    ("from_symbol", self.from_symbol.as_str()),
                    ("to_symbol", self.to_symbol.as_str()),
                    // Always full: see the module note on cursor safety.
                    ("outputsize", "full"),
                    ("apikey", self.api_key.as_str()),
                ],
            )
            .await?;

        let series = check_response(body)?;
        let records = assemble(series, self.next_start, closed_boundary);
        // The response covers the venue's entire published history up to the
        // present, so everything before the boundary has now been seen — there
        // is no unfetched remainder to leave the cursor behind for.
        self.next_start = closed_boundary;
        let cursor = Cursor::new(&FxCursor {
            next_start: self.next_start,
        })?;
        Ok(Batch::new(records).with_cursor(cursor).with_caught_up(true))
    }
}

/// Unwrap the envelope, turning a refusal reported inside a 200 into an error.
///
/// This is the load-bearing check on this venue. A 25-request daily budget is
/// exhausted routinely, and the refusal looks like an ordinary 200 — treating
/// it as an empty series would advance the cursor across a day that was never
/// fetched, and the daily bar for it would never be requested again.
fn check_response(body: FxDailyResponse) -> Result<BTreeMap<String, RawBar>> {
    for (label, value) in [
        ("Information", body.information),
        ("Note", body.note),
        ("Error Message", body.error_message),
    ] {
        if let Some(text) = value {
            return Err(anyhow!(
                "Alpha Vantage refused the request ({label}): {text}"
            ));
        }
    }
    body.series
        .ok_or_else(|| anyhow!("Alpha Vantage returned neither a series nor an error"))
}

/// Turn the dated series into the batch's records: keep bars inside
/// `[next_start, closed_boundary)`, oldest-first. An undecodable bar is dropped
/// rather than failing the batch, matching the other candle adapters.
fn assemble(
    series: BTreeMap<String, RawBar>,
    next_start: i64,
    closed_boundary: i64,
) -> Vec<Candle> {
    let mut records: Vec<Candle> = series
        .iter()
        .filter_map(|(date, bar)| decode(date, bar).ok())
        .filter(|c| c.bucket_start >= next_start && c.bucket_start < closed_boundary)
        .collect();
    records.sort_by_key(|c| c.bucket_start);
    records
}

/// Decode one dated bar into the shared [`Candle`] record.
fn decode(date: &str, raw: &RawBar) -> Result<Candle> {
    let price = |field: &str, value: &str| -> Result<f64> {
        value
            .parse::<f64>()
            .with_context(|| format!("Alpha Vantage {field} price {value:?} is not a number"))
    };
    Ok(Candle {
        bucket_start: parse_civil_utc(date)?,
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
    fn the_floor_stays_inside_the_accounts_twenty_five_per_day() {
        // Note what this does and does not prove. The floor yields ≤ 25 requests
        // across a day *within one continuous run*; it cannot bound the account's
        // actual daily spend, because the interval is in-process state that
        // resets on restart (docs/data-feeds.md §10). A rate can be asserted
        // here; a quota cannot.
        let per_day = requests_per_window(MIN_REQUEST_INTERVAL, Duration::from_secs(24 * 3_600));
        assert!(
            per_day <= 25.0,
            "{per_day} requests/day exceeds Alpha Vantage's 25/day account budget"
        );
    }

    /// A captured response: the real envelope shape, trimmed to three days.
    fn captured_response() -> FxDailyResponse {
        serde_json::from_value(serde_json::json!({
            "Meta Data": {
                "1. Information": "Forex Daily Prices (open, high, low, close)",
                "2. From Symbol": "AUD",
                "3. To Symbol": "USD",
                "4. Output Size": "Full size",
                "5. Last Refreshed": "2026-08-13",
                "6. Time Zone": "UTC"
            },
            "Time Series FX (Daily)": {
                "2026-08-13": { "1. open": "0.70600", "2. high": "0.70670",
                                "3. low": "0.70410", "4. close": "0.70540" },
                "2026-08-12": { "1. open": "0.70510", "2. high": "0.70640",
                                "3. low": "0.70480", "4. close": "0.70600" },
                "2026-08-11": { "1. open": "0.70430", "2. high": "0.70560",
                                "3. low": "0.70400", "4. close": "0.70510" }
            }
        }))
        .expect("captured response decodes")
    }

    #[test]
    fn decodes_a_captured_bar_with_zero_volume() {
        let series = check_response(captured_response()).unwrap();
        let got = decode("2026-08-13", &series["2026-08-13"]).unwrap();
        assert_eq!(
            got,
            Candle {
                bucket_start: civil_to_epoch_secs(2026, 8, 13, 0, 0, 0),
                low: 0.704_10,
                high: 0.706_70,
                open: 0.706_00,
                close: 0.705_40,
                volume: 0.0,
            }
        );
    }

    #[test]
    fn assemble_excludes_todays_forming_bar() {
        // The venue publishes the current day's partial bar under today's date;
        // only the closed boundary keeps it out.
        let series = check_response(captured_response()).unwrap();
        let boundary = civil_to_epoch_secs(2026, 8, 13, 0, 0, 0);
        let got = assemble(series, civil_to_epoch_secs(2026, 8, 1, 0, 0, 0), boundary);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(
            times,
            vec![
                civil_to_epoch_secs(2026, 8, 11, 0, 0, 0),
                civil_to_epoch_secs(2026, 8, 12, 0, 0, 0),
            ]
        );
    }

    #[test]
    fn assemble_excludes_bars_before_the_resume_point() {
        let series = check_response(captured_response()).unwrap();
        let got = assemble(
            series,
            civil_to_epoch_secs(2026, 8, 12, 0, 0, 0),
            civil_to_epoch_secs(2026, 8, 14, 0, 0, 0),
        );
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(
            times,
            vec![
                civil_to_epoch_secs(2026, 8, 12, 0, 0, 0),
                civil_to_epoch_secs(2026, 8, 13, 0, 0, 0),
            ]
        );
    }

    #[test]
    fn the_premium_gate_is_an_error_not_an_empty_series() {
        // The exact body a free key gets for a premium endpoint. If this read
        // as "no bars", the cursor would advance across days never fetched.
        let body: FxDailyResponse = serde_json::from_value(serde_json::json!({
            "Information": "Thank you for using Alpha Vantage! This is a \
                            premium endpoint. You may subscribe to any of the \
                            premium plans at https://www.alphavantage.co/premium/ \
                            to instantly unlock all premium endpoints"
        }))
        .unwrap();
        let err = check_response(body).unwrap_err().to_string();
        assert!(err.contains("premium endpoint"), "{err}");
    }

    #[test]
    fn an_exhausted_daily_budget_is_an_error() {
        // 25 requests/day is the whole account, so this arrives routinely.
        let body: FxDailyResponse = serde_json::from_value(serde_json::json!({
            "Note": "Thank you for using Alpha Vantage! You have reached the \
                     25 requests/day limit."
        }))
        .unwrap();
        assert!(check_response(body).is_err());
    }

    #[test]
    fn a_response_with_neither_series_nor_error_is_an_error() {
        // Never observed, but the alternative is an unwrap on an Option that a
        // venue change could start returning as None.
        let body: FxDailyResponse =
            serde_json::from_value(serde_json::json!({ "Meta Data": {} })).unwrap();
        assert!(check_response(body).is_err());
    }

    #[test]
    fn a_healthy_response_yields_the_series() {
        assert_eq!(check_response(captured_response()).unwrap().len(), 3);
    }
}
