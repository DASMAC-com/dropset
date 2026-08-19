// cspell:word denormal
//! The Pyth Hermes FX adapter (docs/data-feeds.md §9) — the **primary** FX
//! anchor, batched across currencies.
//!
//! Hermes' `/v2/updates/price/latest` prices every feed id in one request, so a
//! whole roster rides a single keyless poll. Each reading carries the
//! publisher's **confidence half-width** and its **publish time**, which is what
//! makes this a primary rather than a fallback: the fair-value engine's
//! fresh-but-uncertain regime (market-making.md §1 fm6) needs a confidence the
//! ECB/Frankfurter daily reference simply does not publish.
//!
//! Two decode details the venue forces, both handled in [`parse_pyth`]:
//!
//! - **Quote direction.** Pyth publishes each cross one way only, and for most
//!   of the roster that is `USD/<ccy>` (`<ccy>` per USD) rather than
//!   `<ccy>/USD`. The anchor leg wants USD per fiat unit, so a feed marked
//!   [`PythFeed::invert`] is reciprocated here — and its confidence with it,
//!   `δ(1/p) ≈ δp / p²`, since a half-width does not survive inversion unchanged.
//! - **Scaled integers.** `price` and `conf` arrive as decimal *strings* with a
//!   shared `expo`; the value is `price × 10^expo`.
//!
//! **Publish time is the staleness clock, not receipt time.** Pyth's FX feeds
//! follow the interbank schedule and stop publishing over the weekend, so a
//! consumer that aged readings from when it received them would see a frozen
//! anchor as perpetually fresh — and miss the weekend regime flip entirely
//! (§1 fm2). [`FxQuote::publish_time`] is carried through for exactly that.

use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// The floor between two requests on this venue.
///
/// Hermes documents **10 requests every 10 seconds per IP** across its
/// endpoints, and answers a breach with a 429 for the following 60 seconds.
/// Since this is the *primary* anchor, that penalty is the expensive one in the
/// roster — it darks the anchor leg rather than one basis input — so the floor
/// is raised well clear of the shared client's 250 ms default (~240/min, 24×
/// the tier).
///
/// **1.2 s rather than the 1 s the limit arithmetically permits.** A 1 s floor
/// yields exactly 10 requests per 10 s, i.e. precisely the documented cap with
/// no margin — and "exactly at the cap" is the wrong place to sit for three
/// reasons: whether the venue meters a fixed or a sliding window decides
/// whether a boundary request is the 10th or an 11th, the limit is per **IP**
/// so a second process on the host is already over, and a retry after a
/// transient failure adds a request the arithmetic never counted. 1.2 s is
/// 8.3 per 10 s, which absorbs all three.
///
/// It does not bind today: one request prices every feed and the maker polls
/// every 5 s. It binds if anything ever issues back-to-back requests here.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(1_200);

/// One FX reading from Hermes: the rate in USD per fiat unit, the publisher's
/// symmetric confidence half-width in the same units, and the epoch second the
/// publishers agreed on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxQuote {
    /// USD per one unit of the currency, after any [`PythFeed::invert`].
    pub value: f64,
    /// Symmetric confidence half-width, in the same units as `value`, when the
    /// venue published a usable one.
    ///
    /// `None` means "no confidence notion this tick" — **not** "zero
    /// half-width". The distinction matters because a zero would read as
    /// *perfect certainty* and silently disable the fresh-but-uncertain gate
    /// (§1 fm6) that is this venue's whole reason for outranking the daily
    /// ECB fallback. Hermes does not publish `conf: 0` on a live FX feed, so a
    /// zero (or a negative, or a non-finite) is a malformed field, and the
    /// honest reading of a malformed field is "unknown".
    pub confidence: Option<f64>,
    /// Epoch second the price was published — the staleness clock (see the
    /// module header).
    pub publish_time: i64,
}

/// One roster entry: the caller's key, Hermes' feed id, and which way round the
/// venue quotes the cross.
#[derive(Clone, Debug, PartialEq)]
pub struct PythFeed {
    /// The key the caller wants the reading back under — an ISO 4217 code for
    /// this roster. The venue's own key is `id`; translating here would hide
    /// the mapping from the caller that owns the roster.
    pub key: String,
    /// Hermes' 32-byte feed id, hex, with or without a `0x` prefix.
    pub id: String,
    /// `true` when the feed is published as `USD/<ccy>` and has to be
    /// reciprocated into USD per `<ccy>`. Of the demo roster only EUR and GBP
    /// are quoted the direct way; CHF, IDR, MXN, SGD, and ZAR are not.
    pub invert: bool,
}

impl PythFeed {
    /// A feed already quoted as `<ccy>/USD` — used as published.
    pub fn direct(key: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            id: id.into(),
            invert: false,
        }
    }

    /// A feed published as `USD/<ccy>`, reciprocated into USD per `<ccy>`.
    pub fn inverted(key: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            id: id.into(),
            invert: true,
        }
    }
}

/// A poll [`Source`] over Hermes' batched latest-price endpoint, keyed by the
/// caller's currency codes.
///
/// It deliberately does **not** follow the batched-quote shape (see [`venues`](super)):
/// [`super::Quotes`] carries a bare `f64` per symbol, which would throw away the
/// confidence and publish time that are this venue's whole reason for being the
/// primary tier.
pub struct PythHermesSource {
    http: HttpClient,
    feeds: Vec<PythFeed>,
}

impl PythHermesSource {
    /// Build the source over `base_url` (e.g. `https://hermes.pyth.network`),
    /// batching every feed in `feeds` into each poll.
    pub fn new(base_url: &str, feeds: Vec<PythFeed>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            feeds,
        })
    }

    /// Fetch every feed this source was built with, in one request.
    pub async fn poll(&self) -> Result<HashMap<String, FxQuote>> {
        if self.feeds.is_empty() {
            return Ok(HashMap::new());
        }
        // Hermes takes a repeated `ids[]` parameter, one per feed.
        let mut query: Vec<(&str, &str)> = self
            .feeds
            .iter()
            .map(|f| ("ids[]", f.id.as_str()))
            .collect();
        query.push(("parsed", "true"));
        let body: Value = self
            .http
            .get_json("/v2/updates/price/latest", &query)
            .await?;
        Ok(parse_pyth(&body, &self.feeds))
    }
}

#[async_trait]
impl Source for PythHermesSource {
    type Record = HashMap<String, FxQuote>;
    fn name(&self) -> &str {
        "pyth-hermes"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Hex ids are compared without a `0x` prefix and case-insensitively, since
/// Hermes accepts either form on the way in and answers in lowercase bare hex.
fn id_key(id: &str) -> String {
    id.strip_prefix("0x").unwrap_or(id).to_ascii_lowercase()
}

/// Decode Hermes' `{"parsed":[{"id":…,"price":{"price","conf","expo",…}}]}`
/// response into `caller key → `[`FxQuote`], scaling by `expo`, inverting the
/// feeds that need it, and keeping only positive finite readings.
///
/// A feed Hermes did not answer for is **omitted** rather than erroring: a
/// roster with one unpublished cross still prices the rest.
pub fn parse_pyth(body: &Value, feeds: &[PythFeed]) -> HashMap<String, FxQuote> {
    let mut out = HashMap::new();
    let Some(parsed) = body.get("parsed").and_then(Value::as_array) else {
        return out;
    };
    // One pass over the response, indexed by feed id, so N feeds cost one walk
    // rather than N scans.
    let by_id: HashMap<String, &Value> = parsed
        .iter()
        .filter_map(|e| Some((id_key(e.get("id")?.as_str()?), e)))
        .collect();

    for feed in feeds {
        let Some(entry) = by_id.get(&id_key(&feed.id)) else {
            continue;
        };
        let Some(price) = entry.get("price") else {
            continue;
        };
        // `price` and `conf` are decimal strings sharing one `expo`.
        let (Some(raw), Some(conf), Some(expo)) = (
            price.get("price").and_then(parse_scaled),
            price.get("conf").and_then(parse_scaled),
            price.get("expo").and_then(Value::as_i64),
        ) else {
            continue;
        };
        let publish_time = price
            .get("publish_time")
            .and_then(Value::as_i64)
            .unwrap_or_default();

        // `expo` is decoded as i64 but scales as i32; reject an out-of-range
        // one rather than letting the cast wrap it into a plausible exponent.
        let Ok(expo) = i32::try_from(expo) else {
            continue;
        };
        let scale = 10f64.powi(expo);
        let (mut value, mut confidence) = (raw * scale, conf * scale);
        if feed.invert {
            // USD/<ccy> → USD per <ccy>. The half-width transforms with the
            // reciprocal's derivative: δ(1/p) ≈ δp / p².
            if !(value.is_finite() && value > 0.0) {
                continue;
            }
            // `value²` overflows to infinity for a rate past ~1e154, which
            // would drive the half-width to a *zero* that reads as perfect
            // certainty while the reciprocal lands on a denormal — a garbage
            // rate wearing maximum confidence, and one that every guard below
            // would wave through. A single corrupt `expo` reaches it
            // (`price: "1", expo: 300`), so it is rejected here rather than
            // left to a downstream band check.
            let squared = value * value;
            if !squared.is_finite() {
                continue;
            }
            confidence /= squared;
            value = 1.0 / value;
        }
        if value.is_finite() && value > 0.0 {
            out.insert(
                feed.key.clone(),
                FxQuote {
                    value,
                    // A non-positive or non-finite half-width is malformed, not
                    // "certain" — see the field's docs.
                    confidence: (confidence.is_finite() && confidence > 0.0).then_some(confidence),
                    publish_time,
                },
            );
        }
    }
    out
}

/// Hermes sends the scaled integers as strings; accept a bare JSON number too,
/// so a captured fixture or a future response shape doesn't silently drop out.
fn parse_scaled(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => s.parse::<f64>().ok(),
        other => other.as_f64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::venues::requests_per_window;
    use serde_json::json;

    #[test]
    fn the_floor_stays_inside_hermes_documented_ten_per_ten_seconds() {
        // Asserted strictly inside the cap, not at it: a floor yielding exactly
        // 10 leaves no room for the sliding-vs-fixed window question, a second
        // process on the same IP, or one retry. See MIN_REQUEST_INTERVAL.
        let per_window = requests_per_window(MIN_REQUEST_INTERVAL, Duration::from_secs(10));
        assert!(
            per_window < 10.0,
            "{per_window} requests/10s does not sit strictly inside Hermes' \
             documented 10 — a breach costs a 429 for the next 60s on the \
             primary anchor"
        );
    }

    /// A captured Hermes response: EUR/USD direct, USD/ZAR inverted.
    fn body() -> Value {
        json!({
            "binary": { "encoding": "hex", "data": ["504e4155"] },
            "parsed": [
                {
                    "id": "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
                    "price": {
                        "price": "115290", "conf": "12",
                        "expo": -5, "publish_time": 1786579250
                    },
                    "ema_price": {
                        "price": "115263", "conf": "13",
                        "expo": -5, "publish_time": 1786579250
                    },
                    "metadata": { "slot": 308110841 }
                },
                {
                    "id": "389d889017db82bf42141f23b61b8de938a4e2d156e36312175bebf797f493f1",
                    "price": {
                        "price": "1750000000", "conf": "500000",
                        "expo": -8, "publish_time": 1786579249
                    }
                }
            ]
        })
    }

    #[test]
    fn parses_a_direct_feed_with_its_confidence() {
        let out = parse_pyth(
            &body(),
            &[PythFeed::direct(
                "EUR",
                "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
            )],
        );
        let eur = out["EUR"];
        // 115290 × 10⁻⁵ = 1.15290 USD per EUR, ± 12 × 10⁻⁵.
        assert!((eur.value - 1.152_90).abs() < 1e-12);
        assert!((eur.confidence.unwrap() - 0.000_12).abs() < 1e-15);
        assert_eq!(eur.publish_time, 1_786_579_250);
    }

    #[test]
    fn inverts_a_usd_per_ccy_feed_and_its_half_width() {
        // USD/ZAR published at 17.50 ± 0.005 → 1/17.50 USD per ZAR, with the
        // half-width scaled by 1/p²: 0.005 / 17.5² ≈ 1.632e-5.
        let out = parse_pyth(
            &body(),
            &[PythFeed::inverted(
                "ZAR",
                "389d889017db82bf42141f23b61b8de938a4e2d156e36312175bebf797f493f1",
            )],
        );
        let zar = out["ZAR"];
        assert!((zar.value - 1.0 / 17.5).abs() < 1e-12);
        let conf = zar.confidence.unwrap();
        assert!((conf - 0.005 / (17.5 * 17.5)).abs() < 1e-15);
        // Inversion keeps the fractional half-width invariant to first order,
        // which is what the engine's fx_max_confidence_frac gate compares.
        let direct_frac = 0.005 / 17.5;
        assert!((conf / zar.value - direct_frac).abs() < 1e-12);
    }

    #[test]
    fn matches_ids_regardless_of_prefix_or_case() {
        let out = parse_pyth(
            &body(),
            &[PythFeed::direct(
                "EUR",
                "0xA995D00BB36A63CEF7FD2C287DC105FC8F3D93779F062F09551B0AF3E81EC30B",
            )],
        );
        assert!(out.contains_key("EUR"));
    }

    #[test]
    fn omits_a_feed_hermes_did_not_answer_for() {
        let out = parse_pyth(
            &body(),
            &[
                PythFeed::direct(
                    "EUR",
                    "a995d00bb36a63cef7fd2c287dc105fc8f3d93779f062f09551b0af3e81ec30b",
                ),
                PythFeed::inverted(
                    "IDR",
                    "6693afcd49878bbd622e46bd805e7177932cf6ab0b1c91b135d71151b9207433",
                ),
            ],
        );
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("EUR"));
        assert!(!out.contains_key("IDR"));
    }

    #[test]
    fn drops_a_non_positive_price_instead_of_dividing_by_it() {
        let body = json!({
            "parsed": [{
                "id": "aa", "price": { "price": "0", "conf": "1", "expo": -5, "publish_time": 1 }
            }]
        });
        assert!(parse_pyth(&body, &[PythFeed::inverted("EUR", "aa")]).is_empty());
        assert!(parse_pyth(&body, &[PythFeed::direct("EUR", "aa")]).is_empty());
    }

    #[test]
    fn missing_parsed_array_yields_nothing() {
        assert!(parse_pyth(&json!({ "binary": {} }), &[PythFeed::direct("EUR", "aa")]).is_empty());
    }

    #[test]
    fn accepts_numeric_as_well_as_string_scaled_fields() {
        let body = json!({
            "parsed": [{
                "id": "aa", "price": { "price": 115290, "conf": 12, "expo": -5, "publish_time": 7 }
            }]
        });
        let out = parse_pyth(&body, &[PythFeed::direct("EUR", "aa")]);
        assert!((out["EUR"].value - 1.152_90).abs() < 1e-12);
    }

    #[test]
    fn a_zero_or_negative_half_width_reads_as_unknown_not_certain() {
        // A zero would make `Reading::uncertain` unconditionally false — i.e.
        // the venue could switch off the only gate that widens the spread —
        // so it is carried as "no confidence", which is what it means.
        for conf in ["0", "-1"] {
            let body = json!({
                "parsed": [{
                    "id": "aa",
                    "price": { "price": "115290", "conf": conf, "expo": -5, "publish_time": 7 }
                }]
            });
            let out = parse_pyth(&body, &[PythFeed::direct("EUR", "aa")]);
            // The price still stands — a malformed half-width is not a
            // malformed rate.
            assert!((out["EUR"].value - 1.152_90).abs() < 1e-12);
            assert_eq!(out["EUR"].confidence, None, "conf {conf}");
        }
    }

    #[test]
    fn an_inversion_that_would_overflow_is_rejected_outright() {
        // `price: 1, expo: 300` scales to 1e300; squaring it for the
        // half-width transform overflows to infinity, which would leave a
        // denormal rate carrying a zero (= "perfectly certain") half-width.
        // Both the rate and the confidence are garbage, so the reading goes.
        let body = json!({
            "parsed": [{
                "id": "aa",
                "price": { "price": "1", "conf": "1", "expo": 300, "publish_time": 7 }
            }]
        });
        assert!(parse_pyth(&body, &[PythFeed::inverted("ZAR", "aa")]).is_empty());
    }

    #[test]
    fn an_out_of_range_exponent_is_rejected_rather_than_wrapped() {
        // `expo` decodes as i64 but scales as i32: 4294967291 truncates to -5,
        // which would silently produce a plausible-looking rate.
        let body = json!({
            "parsed": [{
                "id": "aa",
                "price": {
                    "price": "115290", "conf": "12",
                    "expo": 4_294_967_291i64, "publish_time": 7
                }
            }]
        });
        assert!(parse_pyth(&body, &[PythFeed::direct("EUR", "aa")]).is_empty());
    }
}
