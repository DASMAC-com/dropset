// cspell:word OHLC
// cspell:word pointwise
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
//!
//! **The v20 instrument list is direction-fixed, so some pairs are inverted at
//! intake.** OANDA lists exactly one instrument per pair, in market convention,
//! and rejects the other direction outright rather than inverting it —
//! `USD_CAD` exists, `CAD_USD` answers `400`. A collector storing the canonical
//! `CAD-USD` therefore asks for `USD_CAD` and flips every candle here, before
//! it reaches a sink, so `cex_prices` only ever holds canonical-direction
//! values (see the private `invert` field). Inverting a bar is **not** a
//! per-field reciprocal: it swaps high and low, because `x -> 1/x` reverses
//! order.

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
    /// Whether [`Self::instrument`] is the reciprocal of the pair the collector
    /// stores, so every candle is inverted before it leaves this source.
    ///
    /// **Inversion happens here, at intake, so the store only ever holds
    /// canonical-direction values.** The alternative — persisting the venue's
    /// raw series and flipping it downstream — makes a reciprocal row
    /// indistinguishable from a real price in `cex_prices`, so every consumer
    /// (fair value, the dashboards, an analyst's query) would have to know
    /// which product ids to un-invert, and any that forgot would be wrong
    /// quietly.
    invert: bool,
}

impl OandaCandles {
    /// The venue's configured transport.
    ///
    /// Split out from [`OandaCandles::resume`] so a collector polling several
    /// instruments builds **one** client and clones it per feed. That is a
    /// rate-limit requirement rather than a saving: an [`HttpClient`]'s clones
    /// share one request-pacing budget while a second `HttpClient::new` opens
    /// an independent one, so constructing a client per instrument would
    /// multiply the venue's budget by the roster size (docs/data-feeds.md §10).
    ///
    /// `api_key` is the v20 bearer token, taken as an argument rather than read
    /// from the environment here (docs/data-feeds.md §4) — the caller decides
    /// where the secret came from.
    pub fn client(base_url: &str, api_key: &str) -> Result<HttpClient> {
        HttpClient::new(base_url)?
            .with_secret_header("Authorization", &format!("Bearer {api_key}"))?
            .with_header(DATETIME_FORMAT_HEADER, "UNIX")
    }

    /// Build the source over a transport from [`OandaCandles::client`],
    /// resuming from a saved framework cursor when present and otherwise
    /// starting the backfill at `default_start`.
    ///
    /// `invert` marks an instrument this venue quotes the other way round; the
    /// caller takes it from the resolved roster rather than deciding it here,
    /// because only the roster knows which canonical id the instrument stands
    /// for. Every candle is then flipped before it leaves this source, so a
    /// sink only ever sees canonical-direction values.
    #[allow(clippy::too_many_arguments)]
    pub fn resume(
        http: HttpClient,
        name: impl Into<String>,
        instrument: impl Into<String>,
        granularity_secs: i64,
        max_buckets: usize,
        resume: Option<Cursor>,
        default_start: i64,
        invert: bool,
    ) -> Result<Self> {
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
            invert,
        })
    }

    /// The venue instrument this source polls.
    ///
    /// Exposed so a collector can assert what it actually wired, rather than
    /// what it meant to. See [`Self::inverts`].
    pub fn instrument(&self) -> &str {
        &self.instrument
    }

    /// Whether this source inverts each candle before yielding it.
    ///
    /// **This accessor exists because the argument behind it is the one line
    /// in the collector whose failure is silent.** `resume` takes eight
    /// positional arguments and this is the last of them, so a slip that
    /// passed `false` here would leave every test green — the adapter's own
    /// tests construct their sources directly, and the live-venue tests
    /// compose the same call themselves — while the store filled with
    /// reciprocals under canonical product ids. A reader cannot tell that
    /// from the data, because a reciprocal is a plausible price.
    pub fn inverts(&self) -> bool {
        self.invert
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

        let records = assemble(body.candles, self.next_start, end, self.invert);
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
/// ascending records), inverting each one when the instrument is the
/// reciprocal of the stored pair.
///
/// A candle that fails to decode is dropped rather than failing the batch. The
/// alternative would let one malformed row stall a backfill indefinitely, and
/// the store's `ON CONFLICT DO NOTHING` means a later re-fetch can still fill
/// the gap.
///
/// **A candle that fails to *invert* is dropped too, but the inherited
/// rationale does not transfer intact and it is worth being exact about why.**
/// A decode failure is a per-row malformation: one bad string in one candle,
/// incidental and self-clearing. An inversion failure is a property of the
/// *value*, and the value comes from the same series every window — so a venue
/// state that produces one produces them all, in every window. Combined with
/// the cursor advancing past the whole window regardless, a systematic
/// inversion failure would render as a permanently empty series, which reads
/// as *market closed* rather than as an error, and the `ON CONFLICT` mitigation
/// does not apply because nothing rewinds the cursor.
///
/// Dropping is still the right call — one bar must not stall a backfill, and
/// `decode`'s floor means reaching this at all takes an out-of-contract
/// response. But the drop is **logged** rather than silent, because the silence
/// watch that would eventually notice an empty series cannot say *why* it is
/// empty, and the error discarded here is the only thing that can.
fn assemble(raw: Vec<RawCandle>, next_start: i64, end: i64, invert: bool) -> Vec<Candle> {
    let mut records: Vec<Candle> = raw
        .into_iter()
        .filter(|c| c.complete)
        .filter_map(|c| {
            // Logged for the same reason the inversion drop below is, and the
            // reason now applies more strongly here: `decode` enforces the
            // positive-finite floor, so a venue emitting `NaN`, `inf` or a
            // negative does it every window — the recurring, renders-as-closed
            // shape — and this is the branch every canonical-direction pair
            // takes.
            decode(&c)
                .inspect_err(|err| {
                    tracing::warn!(
                        time = %c.time,
                        error = %err,
                        "dropping a candle that could not be decoded"
                    );
                })
                .ok()
        })
        .filter_map(|c| {
            if invert {
                invert_candle(&c)
                    .inspect_err(|err| {
                        tracing::warn!(
                            bucket_start = c.bucket_start,
                            error = %err,
                            "dropping a candle that could not be inverted"
                        );
                    })
                    .ok()
            } else {
                Some(c)
            }
        })
        .filter_map(|c| {
            // The ordering invariant, checked **after** any inversion — which is
            // the only place it can be. `decode`'s floor judges one field at a
            // time, so it cannot see a relation between two of them, and
            // inversion is where the relation is at risk: `x -> 1/x` reverses
            // high and low, so an inversion that failed to swap them yields
            // exactly the bar `cex_prices` refuses on its ordering constraint.
            //
            let bucket_start = c.bucket_start;
            c.validated()
                .inspect_err(|err| {
                    tracing::warn!(
                        venue = "oanda",
                        bucket_start,
                        error = %err,
                        "dropping a candle that is not storable"
                    );
                })
                .ok()
        })
        .filter(|c| c.bucket_start >= next_start && c.bucket_start < end)
        .collect();
    records.sort_by_key(|c| c.bucket_start);
    records
}

/// One price, the other way round.
///
/// Rejects a value that cannot be inverted into a price rather than producing
/// one: `1.0 / 0.0` is `inf` in release, and an infinity reaching `cex_prices`
/// is a `NaN`-shaped poison in every average computed over it afterwards.
///
/// **[`decode`] already enforces a positive finite floor, so the first check
/// here is defense in depth rather than the only guard** — this function is
/// reachable from tests and from any future caller that has not been through
/// `decode`. The *second* check is not redundant at all, and is the one that
/// earns its keep: a subnormal input (`1e-320`) is finite and positive, clears
/// every check `decode` makes, and still overflows to `inf` when inverted.
fn invert_price(price: f64) -> Result<f64> {
    if !price.is_finite() || price <= 0.0 {
        return Err(anyhow!(
            "cannot invert the price {price}: an inverted quote needs a finite \
             positive value"
        ));
    }
    let inverted = 1.0 / price;
    if !inverted.is_finite() {
        return Err(anyhow!("inverting the price {price} overflowed"));
    }
    Ok(inverted)
}

/// Invert a whole candle, for an instrument quoted opposite to the pair being
/// stored.
///
/// **The extremes swap, and that is the part a per-field reciprocal gets
/// wrong.** `x -> 1/x` is order-reversing on positive reals, so the *highest*
/// price of the raw series is the *lowest* price of the inverted one: the new
/// high comes from the raw low. Open and close are single points in time and so
/// inverting them pointwise is correct — it is only the two order statistics
/// that trade places. A naive field-wise flip yields `high < low`, and nothing
/// downstream would catch it: `cex_prices` stores the four as independent
/// columns with no ordering constraint.
///
/// `volume` is carried through untouched. OANDA's is a **tick count** — how
/// many quotes arrived in the bucket — which is a property of the market's
/// activity, not of the direction it is quoted in, so it needs no transform
/// (and inverting it would be meaningless).
fn invert_candle(raw: &Candle) -> Result<Candle> {
    Ok(Candle {
        bucket_start: raw.bucket_start,
        // Crossed on purpose: see the note above.
        low: invert_price(raw.high)?,
        high: invert_price(raw.low)?,
        open: invert_price(raw.open)?,
        close: invert_price(raw.close)?,
        volume: raw.volume,
    })
}

/// Decode one complete candle into the shared [`Candle`] record.
///
/// **Parsing is not validation, so this also enforces a positive finite
/// floor.** `str::parse::<f64>()` accepts `"inf"`, `"-inf"`, `"NaN"`, `"-0"`
/// and negatives as `Ok`, so a parse alone leaves the value sane only because
/// the venue is *trusted* to send sane numbers. `cex_prices` constrains its
/// price columns to `NOT NULL` and nothing more — no `CHECK` — so an infinity
/// or a `NaN` that gets this far is stored, and then poisons every average
/// taken over the series afterwards.
///
/// The floor lives **here**, at the one place the venue's data becomes a
/// `Candle`, rather than beside the inversion — otherwise it would guard only
/// the reciprocal pairs and leave every canonical-direction pair (the majority
/// of any roster) unchecked, which is validation that varies by quote
/// direction for no reason.
///
/// **One narrow asymmetry survives, deliberately.** A subnormal price
/// (`1e-320`) is finite and positive, so it clears this floor and is stored on
/// a canonical-direction pair — while on a reversed pair it is dropped,
/// because its reciprocal overflows. That is a property of the arithmetic
/// rather than an unchecked path: there is nothing wrong with a subnormal as a
/// *stored* price, only as one about to be inverted. Stated here so the
/// remaining difference is on the record rather than looking like the same
/// oversight this floor just fixed.
fn decode(raw: &RawCandle) -> Result<Candle> {
    let price = |field: &str, value: &str| -> Result<f64> {
        let parsed = value
            .parse::<f64>()
            .with_context(|| format!("OANDA {field} price {value:?} is not a number"))?;
        if !parsed.is_finite() || parsed <= 0.0 {
            return Err(anyhow!(
                "OANDA {field} price {value:?} is not a positive finite number"
            ));
        }
        Ok(parsed)
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
        let got = assemble(body.candles, 1_786_668_600, 1_786_668_780, false);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(times, vec![1_786_668_660]);
    }

    /// An inverted bar is dropped rather than stored, even though every one of
    /// its prices clears `decode`'s per-field floor.
    ///
    /// This is the gap the per-field floor cannot cover by construction: it
    /// judges one value at a time, so a bar whose four prices are each perfectly
    /// good but whose high sits below its low passes it completely. `cex_prices`
    /// refuses such a bar on its ordering constraint, and this adapter is the
    /// reason that constraint exists — inversion has to swap high and low.
    #[test]
    fn assemble_drops_a_bar_whose_high_is_below_its_low() {
        let body: CandlesResponse = serde_json::from_value(serde_json::json!({
            "instrument": "AUD_USD",
            "granularity": "M1",
            "candles": [
                {
                    // Every price is finite and positive, so `decode` accepts
                    // all four; only their relation is wrong.
                    "complete": true,
                    "volume": 28,
                    "time": "1786668660.000000000",
                    "mid": { "o": "0.70604", "h": "0.70600",
                             "l": "0.70602", "c": "0.70601" }
                },
                {
                    "complete": true,
                    "volume": 31,
                    "time": "1786668720.000000000",
                    "mid": { "o": "0.70604", "h": "0.70606",
                             "l": "0.70602", "c": "0.70606" }
                }
            ]
        }))
        .expect("the fixture decodes");

        // Sanity: the bad bar really does clear the per-field floor, so this
        // test would pass vacuously if it were rejected earlier.
        assert!(
            decode(&body.candles[0]).is_ok(),
            "the fixture must exercise the ordering check, not the value floor"
        );

        let got = assemble(body.candles, 1_786_668_600, 1_786_668_780, false);
        let times: Vec<i64> = got.iter().map(|c| c.bucket_start).collect();
        assert_eq!(
            times,
            vec![1_786_668_720],
            "the inverted bar must be dropped on its own, leaving the sound one"
        );
    }

    #[test]
    fn assemble_excludes_candles_outside_the_requested_window() {
        // Defensive on both ends: nothing before the resume point and nothing
        // at or past the window end leaks into the batch.
        let body = captured_response();
        assert!(assemble(body.candles, 1_786_668_700, 1_786_668_800, false).is_empty());
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
        assert!(assemble(body.candles, 1_786_147_200, 1_786_233_600, false).is_empty());
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
        let http = OandaCandles::client("https://example.test", "token").unwrap();
        let source = OandaCandles::resume(
            http,
            "fx:oanda:AUD-USD",
            "AUD_USD",
            60,
            10_000,
            None,
            1_000,
            false,
        )
        .unwrap();
        assert_eq!(source.max_buckets, MAX_CANDLES_PER_REQUEST);
    }

    #[test]
    fn an_unsupported_granularity_is_rejected_at_construction() {
        // Not merely mapped wrong later: the source must refuse to exist.
        let http = OandaCandles::client("https://example.test", "token").unwrap();
        assert!(OandaCandles::resume(
            http,
            "fx:oanda:AUD-USD",
            "AUD_USD",
            180,
            500,
            None,
            1_000,
            false,
        )
        .is_err());
    }

    #[test]
    fn one_client_serves_a_whole_roster_on_a_single_rate_budget() {
        // The invariant a roster collector depends on: several instruments
        // share one transport, so the venue's request budget is not multiplied
        // by the number of pairs polled.
        let http = OandaCandles::client("https://example.test", "token").unwrap();
        for instrument in ["AUD_USD", "EUR_USD", "GBP_USD"] {
            assert!(OandaCandles::resume(
                http.clone(),
                format!("fx:oanda:{instrument}"),
                instrument,
                60,
                500,
                None,
                1_000,
                false,
            )
            .is_ok());
        }
    }

    /// One raw bar with four distinct, ordered prices, so an inversion that
    /// flips fields in place is distinguishable from one that swaps the
    /// extremes.
    fn raw_bar() -> Candle {
        Candle {
            bucket_start: 1_786_668_660,
            low: 1.37000,
            high: 1.38000,
            open: 1.37200,
            close: 1.37838,
            volume: 28.0,
        }
    }

    #[test]
    fn an_inverted_candle_swaps_the_extremes_rather_than_flipping_each_field() {
        // The bug this exists to catch, and the reason it is worth a test of
        // its own: `x -> 1/x` reverses order, so a field-wise reciprocal puts
        // the reciprocal of the raw high into `high`, which is the *smallest*
        // of the four. The result is a bar with high < low, and nothing
        // downstream would reject it — `cex_prices` holds the four as
        // independent columns with no ordering constraint, so it would sit
        // there looking like data.
        let got = invert_candle(&raw_bar()).unwrap();

        assert_eq!(
            got.high,
            1.0 / 1.37000,
            "the new high comes from the raw low"
        );
        assert_eq!(
            got.low,
            1.0 / 1.38000,
            "the new low comes from the raw high"
        );
        assert!(
            got.high > got.low,
            "inverted bar must stay ordered: high {} low {}",
            got.high,
            got.low
        );

        // Open and close are points in time, not order statistics, so they
        // invert pointwise and do not trade places.
        assert_eq!(got.open, 1.0 / 1.37200);
        assert_eq!(got.close, 1.0 / 1.37838);

        // Untouched: OANDA's volume is a tick count, which the quote direction
        // does not affect.
        assert_eq!(got.volume, 28.0);
        assert_eq!(got.bucket_start, 1_786_668_660);
    }

    #[test]
    fn inversion_reproduces_the_measured_cross_venue_agreement() {
        // The measurement that motivated the whole change (2026-09-08): OANDA
        // serves USD_CAD at 1.37838 and Twelve Data serves CAD/USD at 0.7255.
        // Those are the same market seen from two directions, so inverting the
        // former must land on the latter — that agreement is what says the
        // stored series is genuinely canonical and not merely relabelled.
        let inverted = invert_price(1.37838).unwrap();
        assert!(
            (inverted - 0.7255).abs() < 5e-5,
            "1/1.37838 = {inverted}, expected Twelve Data's 0.7255"
        );
    }

    #[test]
    fn a_price_that_cannot_be_inverted_is_refused_rather_than_made_infinite() {
        // `1.0 / 0.0` is `inf` rather than a panic, and an infinity in
        // `cex_prices` poisons every average taken over it afterwards, so the
        // refusal has to happen here.
        assert!(invert_price(0.0).is_err());
        assert!(invert_price(-1.5).is_err());
        assert!(invert_price(f64::NAN).is_err());
        assert!(invert_price(f64::INFINITY).is_err());

        // The overflow re-check, which the four cases above do NOT reach —
        // every one of them fails the first guard, so without this the second
        // guard could be deleted with the suite still green. A subnormal is
        // finite and positive, so it clears every check `decode` makes, and
        // its reciprocal overflows.
        assert!(invert_price(1e-320).is_err());
        assert!(invert_price(5e-324).is_err(), "the smallest subnormal");

        // ...and the bound, so the guard is not mistaken for "rejects small
        // values": `f64::MIN_POSITIVE` inverts to a large but finite number,
        // so it is accepted. Only inputs whose reciprocal actually overflows
        // are refused.
        assert!(invert_price(f64::MIN_POSITIVE).is_ok());
    }

    #[test]
    fn assemble_inverts_only_when_the_instrument_is_reversed() {
        // The flag has to reach `assemble`, not just exist: the same captured
        // response must come back raw or inverted depending on it.
        let body = captured_response();
        let direct = assemble(body.candles, 1_786_668_600, 1_786_668_780, false);
        let body = captured_response();
        let inverted = assemble(body.candles, 1_786_668_600, 1_786_668_780, true);

        assert_eq!(direct.len(), 1);
        assert_eq!(inverted.len(), 1);
        assert_eq!(inverted[0].bucket_start, direct[0].bucket_start);
        assert_eq!(inverted[0].close, 1.0 / direct[0].close);
        assert_eq!(inverted[0].high, 1.0 / direct[0].low);
        assert!(inverted[0].high > inverted[0].low);
    }

    #[test]
    fn a_non_finite_price_is_dropped_on_the_direct_path_too_not_only_inverted() {
        // The point of putting the floor in `decode` rather than beside the
        // inversion: `str::parse::<f64>()` accepts all four of these as `Ok`,
        // so before the floor existed each one would have reached the store on
        // any non-inverted pair — which is every pair on a default roster
        // except CAD-USD. Guarding only the inverted path would have made
        // validation depend on quote direction.
        //
        // Stated without appeal to what the column does or does not enforce:
        // the schema gained its own price CHECKs later, so an argument resting
        // on their absence would have expired. The floor earns its place by
        // being the layer that can drop one bar instead of failing the batch.
        for bad in ["0", "-1.5", "inf", "NaN"] {
            let body: CandlesResponse = serde_json::from_value(serde_json::json!({
                "instrument": "AUD_USD",
                "granularity": "M1",
                "candles": [{
                    "complete": true,
                    "volume": 7,
                    "time": "1786668660.000000000",
                    "mid": { "o": "0.65", "h": "0.66", "l": "0.64", "c": bad }
                }]
            }))
            .unwrap();
            // invert = FALSE — the direction that had no guard at all.
            let got = assemble(body.candles, 1_786_668_600, 1_786_668_780, false);
            assert!(
                got.is_empty(),
                "a {bad:?} close must not reach the store on the direct path"
            );
        }
    }

    #[test]
    fn a_candle_that_cannot_be_inverted_is_dropped_without_taking_the_batch() {
        // The drop policy documented on `assemble` had no test, and the
        // plausible wrong edit — `.ok().or(Some(c))`, "don't lose data" —
        // would have kept every other test green while letting an
        // *un-inverted* reciprocal reach `cex_prices`. This pins it.
        //
        // Note which value is needed: `decode`'s positive-finite floor now
        // rejects "0", "-1", "inf" and "NaN" before the inversion is ever
        // reached, so the only input that decodes cleanly and *then* fails to
        // invert is a subnormal, whose reciprocal overflows.
        let body: CandlesResponse = serde_json::from_value(serde_json::json!({
            "instrument": "USD_CAD",
            "granularity": "M1",
            "candles": [
                {
                    "complete": true,
                    "volume": 11,
                    "time": "1786668660.000000000",
                    "mid": {
                        "o": "1e-320", "h": "1e-320",
                        "l": "1e-320", "c": "1e-320"
                    }
                },
                {
                    "complete": true,
                    "volume": 22,
                    "time": "1786668720.000000000",
                    "mid": {
                        "o": "1.37200", "h": "1.38000",
                        "l": "1.37000", "c": "1.37838"
                    }
                }
            ]
        }))
        .unwrap();

        let got = assemble(body.candles, 1_786_668_600, 1_786_668_780, true);

        // The healthy candle survives; the un-invertible one is gone rather
        // than passed through raw.
        assert_eq!(got.len(), 1, "only the invertible candle should survive");
        assert_eq!(got[0].bucket_start, 1_786_668_720);
        assert_eq!(got[0].volume, 22.0);
        assert!(
            got[0].close < 1.0,
            "the survivor must be inverted, not raw: {}",
            got[0].close
        );
    }
}
