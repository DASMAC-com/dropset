// cspell:word OHLC
//! The OANDA v20 candles adapter (docs/data-feeds.md §9) — the **FX anchor**,
//! and the first source whose subject is a real currency pair rather than a
//! stablecoin proxy.
//!
//! It polls the v20 practice endpoint `/v3/instruments/{instrument}/candles`,
//! authenticated with a bearer token supplied by the caller. Two things make it
//! the primary FX tier rather than another fallback: the free practice tier
//! serves **years** of minute history (verified back three years), so a cold
//! collector can backfill as deep as its consumers need; and the venue marks
//! each candle `complete`, so closed-bucket discipline comes from the venue
//! rather than from clock arithmetic.
//!
//! **Timestamps arrive as epoch seconds, not RFC3339.** The adapter sends
//! `Accept-Datetime-Format: UNIX`, which makes `time` a `"1786668660.000000000"`
//! string instead of `"2026-08-14T00:51:00.000000000Z"`. That is deliberate:
//! `cex_prices.bucket_start` is an epoch-second `BIGINT` precisely to keep the
//! collectors free of a chrono/time dependency, and parsing RFC3339 here would
//! reintroduce one for no gain.
//!
//! **The FX market closes on weekends**, so a window inside one legitimately
//! returns zero candles. The cursor advances past the whole requested window
//! rather than to the newest row returned — an empty weekend must move the
//! position or a backfill stalls on it forever. This is the same discipline as
//! the Coinbase adapter's, and for the same reason.
//!
//! Like Coinbase's, this endpoint is keyed by a single instrument, so it is
//! deliberately **not** a batched quote venue (see [`venues`](super)): one source covers one
//! pair, and a roster is several sources rather than one batched poll.

use super::Candle;
use crate::time::now_secs;
use crate::{Batch, Cursor, HttpClient, Source};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// OANDA's per-request candle cap. A request for more is rejected outright
/// (`Maximum value for 'count' exceeded`), so a backfill pages in windows no
/// wider. It is the venue's constraint, so it lives with the venue.
pub const MAX_CANDLES_PER_REQUEST: usize = 5000;

/// The canonical name of this venue's credential
/// ([`crate::secrets`]) — the v20 personal access token.
pub const SECRET_NAME: &str = "oanda/api-key";

// This source declines to raise its request floor, and the number is recorded
// here so the next pager inherits it rather than rediscovering it. OANDA allows
// **100 requests per second**; the shared client's 250 ms default is 4 a second,
// already ~25× stricter than the venue asks, so `with_min_interval` would buy
// nothing (docs/data-feeds.md §10). This venue and Coinbase are the only two in
// the crate that keep the default, and both keep it for this same reason: the
// documented rate is higher than the default permits.
//
// A plain comment rather than a doc comment because there is no *floor* constant
// to attach it to — the absence of one is precisely the point.

/// The header that switches every timestamp in the response — and every
/// timestamp accepted in a query — from RFC3339 to epoch seconds.
const DATETIME_FORMAT_HEADER: &str = "Accept-Datetime-Format";

/// The price component to fetch: `M` yields the mid, which is the reference
/// rate a fair-value engine wants. Bid/ask are available (`B` / `A`) and would
/// be a separate source if a consumer ever needs the spread.
const PRICE_COMPONENT: &str = "M";

/// This source's opaque resume position: the next epoch second still to fetch.
/// Structurally identical to the Coinbase source's, and deliberately its own
/// type — two feeds' cursors are stored under different keys and nothing should
/// make it easy to read one as the other.
#[derive(Serialize, Deserialize)]
struct FxCursor {
    next_start: i64,
}

/// One candle as v20 returns it under `Accept-Datetime-Format: UNIX`.
#[derive(Debug, Deserialize)]
struct RawCandle {
    /// False for the currently-forming candle, which must never be persisted.
    complete: bool,
    /// Tick count for the bucket. OANDA has no notion of traded size on a
    /// practice feed, so this is activity, not volume in the CEX sense.
    volume: f64,
    /// Epoch seconds with a fractional part, e.g. `"1786668660.000000000"`.
    time: String,
    mid: RawPrices,
}

/// The OHLC quartet, which v20 sends as strings.
#[derive(Debug, Deserialize)]
struct RawPrices {
    o: String,
    h: String,
    l: String,
    c: String,
}

/// The candles response envelope.
#[derive(Debug, Deserialize)]
struct CandlesResponse {
    candles: Vec<RawCandle>,
}

/// A poll [`Source`] over one OANDA instrument's candles.
pub struct OandaCandles {
    http: HttpClient,
    name: String,
    /// The venue's own symbol (`AUD_USD`), which is **not** the canonical
    /// `product_id` a collector stores. Keeping the two separate is what lets
    /// four vendors with four different spellings land under one stored symbol.
    instrument: String,
    granularity_secs: i64,
    /// The venue's granularity token (`M1`), derived once at construction.
    granularity_code: &'static str,
    max_buckets: usize,
    /// The oldest epoch second not yet persisted; advances as windows drain.
    next_start: i64,
}

impl OandaCandles {
    /// Build the source, resuming from a saved framework cursor when present
    /// and otherwise starting the backfill at `default_start`.
    ///
    /// `api_key` is the v20 bearer token, taken as an argument rather than read
    /// from the environment here (docs/data-feeds.md §4) — the caller decides
    /// where the secret came from.
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        base_url: &str,
        api_key: &str,
        name: impl Into<String>,
        instrument: impl Into<String>,
        granularity_secs: i64,
        max_buckets: usize,
        resume: Option<Cursor>,
        default_start: i64,
    ) -> Result<Self> {
        let http = HttpClient::new(base_url)?
            .with_secret_header("Authorization", &format!("Bearer {api_key}"))?
            .with_header(DATETIME_FORMAT_HEADER, "UNIX")?;
        let next_start = match resume {
            Some(cursor) => cursor.get::<FxCursor>()?.next_start,
            None => default_start,
        };
        Ok(Self {
            http,
            name: name.into(),
            instrument: instrument.into(),
            granularity_secs,
            granularity_code: granularity_code(granularity_secs)?,
            max_buckets: max_buckets.clamp(1, MAX_CANDLES_PER_REQUEST),
            next_start,
        })
    }

    /// The start of the currently-forming bucket. The venue's `complete` flag
    /// is what actually excludes a forming candle; this only bounds how far a
    /// request reaches, so the backfill never asks for the future.
    fn closed_boundary(&self) -> i64 {
        let now = now_secs();
        now - now.rem_euclid(self.granularity_secs)
    }
}

#[async_trait]
impl Source for OandaCandles {
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
            self.max_buckets,
            closed_boundary,
        );
        let from_s = self.next_start.to_string();
        let to_s = end.to_string();
        let path = format!("/v3/instruments/{}/candles", self.instrument);
        let body: CandlesResponse = self
            .http
            .get_json(
                &path,
                &[
                    ("granularity", self.granularity_code),
                    ("from", from_s.as_str()),
                    ("to", to_s.as_str()),
                    ("price", PRICE_COMPONENT),
                ],
            )
            .await?;

        let records = assemble(body.candles, self.next_start, end);
        // Advance past the whole requested window, not to the newest row: a
        // weekend window returns nothing at all, and anchoring on the last row
        // would leave the cursor parked in front of it forever.
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

/// Map a bucket width in seconds onto v20's granularity token.
///
/// An explicit allowlist rather than a computed string, because the venue is
/// **not** a reliable validator here: a probe of the undocumented `M3` came
/// back `200` with `"granularity": "M3"` echoed and a candle attached, so an
/// unsupported width would silently produce buckets of some other size. Failing
/// in the constructor is the only place this can be caught cheaply.
fn granularity_code(secs: i64) -> Result<&'static str> {
    Ok(match secs {
        5 => "S5",
        10 => "S10",
        15 => "S15",
        30 => "S30",
        60 => "M1",
        120 => "M2",
        240 => "M4",
        300 => "M5",
        600 => "M10",
        900 => "M15",
        1_800 => "M30",
        3_600 => "H1",
        7_200 => "H2",
        10_800 => "H3",
        14_400 => "H4",
        21_600 => "H6",
        28_800 => "H8",
        43_200 => "H12",
        86_400 => "D",
        604_800 => "W",
        other => {
            return Err(anyhow!(
                "OANDA has no candle granularity of {other}s; supported widths \
                 are 5/10/15/30s, 1/2/4/5/10/15/30m, 1/2/3/4/6/8/12h, 1d, 1w"
            ))
        }
    })
}

/// Decode v20's epoch-second timestamp string (`"1786668660.000000000"`).
///
/// The fractional part is dropped rather than rounded: a candle's `time` is its
/// bucket **open**, which is always a whole second on every granularity the
/// venue offers, so the fraction is formatting rather than information.
fn parse_unix_seconds(time: &str) -> Result<i64> {
    let whole = time.split_once('.').map_or(time, |(secs, _frac)| secs);
    whole
        .parse::<i64>()
        .with_context(|| format!("OANDA timestamp {time:?} is not epoch seconds"))
}

/// The end of the next backfill window: at most `max_buckets` past
/// `next_start`, clamped to the last closed boundary so a request never exceeds
/// the venue's per-request cap nor reaches into the forming bucket.
fn window_end(
    next_start: i64,
    granularity_secs: i64,
    max_buckets: usize,
    closed_boundary: i64,
) -> i64 {
    let span = granularity_secs * max_buckets as i64;
    (next_start + span).min(closed_boundary)
}

/// Turn a raw response into the batch's records: keep only **complete** candles
/// inside `[next_start, end)`, decoded oldest-first (the store sink expects
/// ascending records).
///
/// A candle that fails to decode is dropped rather than failing the batch. The
/// alternative would let one malformed row stall a backfill indefinitely, and
/// the store's `ON CONFLICT DO NOTHING` means a later re-fetch can still fill
/// the gap.
fn assemble(raw: Vec<RawCandle>, next_start: i64, end: i64) -> Vec<Candle> {
    let mut records: Vec<Candle> = raw
        .into_iter()
        .filter(|c| c.complete)
        .filter_map(|c| decode(&c).ok())
        .filter(|c| c.bucket_start >= next_start && c.bucket_start < end)
        .collect();
    records.sort_by_key(|c| c.bucket_start);
    records
}

/// Decode one complete candle into the shared [`Candle`] record.
fn decode(raw: &RawCandle) -> Result<Candle> {
    let price = |field: &str, value: &str| -> Result<f64> {
        value
            .parse::<f64>()
            .with_context(|| format!("OANDA {field} price {value:?} is not a number"))
    };
    Ok(Candle {
        bucket_start: parse_unix_seconds(&raw.time)?,
        low: price("low", &raw.mid.l)?,
        high: price("high", &raw.mid.h)?,
        open: price("open", &raw.mid.o)?,
        close: price("close", &raw.mid.c)?,
        volume: raw.volume,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A captured v20 response: two AUD_USD M1 candles under
    /// `Accept-Datetime-Format: UNIX`, the second of which is still forming.
    fn captured_response() -> CandlesResponse {
        serde_json::from_value(serde_json::json!({
            "instrument": "AUD_USD",
            "granularity": "M1",
            "candles": [
                {
                    "complete": true,
                    "volume": 28,
                    "time": "1786668660.000000000",
                    "mid": { "o": "0.70604", "h": "0.70606",
                             "l": "0.70602", "c": "0.70606" }
                },
                {
                    "complete": false,
                    "volume": 11,
                    "time": "1786668720.000000000",
                    "mid": { "o": "0.70605", "h": "0.70606",
                             "l": "0.70605", "c": "0.70606" }
                }
            ]
        }))
        .expect("captured response decodes")
    }

    #[test]
    fn decodes_a_captured_candle_into_the_shared_record() {
        let body = captured_response();
        let got = decode(&body.candles[0]).unwrap();
        assert_eq!(
            got,
            Candle {
                bucket_start: 1_786_668_660,
                low: 0.706_02,
                high: 0.706_06,
                open: 0.706_04,
                close: 0.706_06,
                volume: 28.0,
            }
        );
    }

    #[test]
    fn assemble_drops_the_forming_candle() {
        // The venue's own `complete` flag is what excludes it, so this holds
        // even though the forming candle sits inside the requested window.
        let body = captured_response();
        let got = assemble(body.candles, 1_786_668_600, 1_786_668_780);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(times, vec![1_786_668_660]);
    }

    #[test]
    fn assemble_excludes_candles_outside_the_requested_window() {
        // Defensive on both ends: nothing before the resume point and nothing
        // at or past the window end leaks into the batch.
        let body = captured_response();
        assert!(assemble(body.candles, 1_786_668_700, 1_786_668_800).is_empty());
    }

    #[test]
    fn a_weekend_window_yields_no_records_rather_than_an_error() {
        // FX is closed Saturday, so v20 answers 200 with an empty candle list.
        // That must be an ordinary empty batch — the caller advances its cursor
        // past the window regardless, which is what keeps a backfill moving.
        let body: CandlesResponse = serde_json::from_value(serde_json::json!({
            "instrument": "AUD_USD", "granularity": "M1", "candles": []
        }))
        .unwrap();
        assert!(assemble(body.candles, 1_786_147_200, 1_786_233_600).is_empty());
    }

    #[test]
    fn timestamps_decode_from_epoch_seconds_with_a_fraction() {
        assert_eq!(
            parse_unix_seconds("1786668660.000000000").unwrap(),
            1_786_668_660
        );
        // The header is what produces the fraction; a bare integer is valid too.
        assert_eq!(parse_unix_seconds("1786668660").unwrap(), 1_786_668_660);
        // An RFC3339 timestamp means the header was dropped somewhere, which
        // must be loud rather than silently becoming a bogus bucket.
        assert!(parse_unix_seconds("2026-08-14T00:51:00.000000000Z").is_err());
    }

    #[test]
    fn granularity_maps_the_widths_the_venue_actually_serves() {
        assert_eq!(granularity_code(60).unwrap(), "M1");
        assert_eq!(granularity_code(900).unwrap(), "M15");
        assert_eq!(granularity_code(86_400).unwrap(), "D");
    }

    #[test]
    fn an_unsupported_granularity_fails_locally_rather_than_at_the_venue() {
        // 180s (M3) is the motivating case: the venue answers 200 for it and
        // echoes the token back, so nothing downstream would notice.
        let err = granularity_code(180).unwrap_err().to_string();
        assert!(err.contains("180s"), "{err}");
    }

    #[test]
    fn window_caps_at_the_bucket_budget_mid_backfill() {
        let end = window_end(1_000, 60, 5_000, 10_000_000);
        assert_eq!(end, 1_000 + 60 * 5_000);
    }

    #[test]
    fn window_clamps_to_the_closed_boundary_near_the_present() {
        let end = window_end(1_000, 60, 5_000, 1_600);
        assert_eq!(end, 1_600);
    }

    #[test]
    fn resume_clamps_an_oversized_window_to_the_venue_cap() {
        let source = OandaCandles::resume(
            "https://example.test",
            "token",
            "fx:oanda:AUD-USD",
            "AUD_USD",
            60,
            10_000,
            None,
            1_000,
        )
        .unwrap();
        assert_eq!(source.max_buckets, MAX_CANDLES_PER_REQUEST);
    }

    #[test]
    fn an_unsupported_granularity_is_rejected_at_construction() {
        // Not merely mapped wrong later: the source must refuse to exist.
        assert!(OandaCandles::resume(
            "https://example.test",
            "token",
            "fx:oanda:AUD-USD",
            "AUD_USD",
            180,
            500,
            None,
            1_000,
        )
        .is_err());
    }
}
