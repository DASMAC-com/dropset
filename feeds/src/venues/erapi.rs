//! The open er-api `/v6/latest` adapter — the roster's widest keyless FX
//! source, batched across currencies in one request.
//!
//! The API quotes `<ccy>` per USD; each reading is **inverted** to USD per
//! `<ccy>`, matching [`frankfurter`](super::frankfurter) and the unit the
//! fair-value engine's anchor leg expects.
//!
//! **Why it earns a slot next to the existing keyless daily source.** It is not
//! another copy of the ECB fix: the provider documents that it blends central
//! banks *and* commercial sources and will not list a currency code without at
//! least three upstream sources, so it is a differently-constructed estimate
//! rather than a second render of the same observation. Concretely it covers
//! **NGN**, the one roster currency neither Pyth Hermes nor the ECB set carries
//! — Hermes catalogues NGN but has never published a price for it, and the ECB
//! reference set omits it entirely, which left it on two vendors alone.
//!
//! **Licence — internal use only, and this bounds where the data may go.** The
//! open-access endpoint permits caching and commercial currency-conversion use
//! but **prohibits re-distribution**. Storing readings here and consuming them
//! to compute a fair value is squarely the permitted use; surfacing them *raw*
//! through a public API or an externally shared dashboard is not, and both read
//! the same store. That is a constraint on any new read surface, not a
//! convention this module can enforce. The endpoint also requires attribution
//! wherever the rates are shown.

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// The floor between two requests on this venue.
///
/// Like [`frankfurter`](super::frankfurter), this venue **publishes no rate
/// limit** — its docs state the open-access endpoint is limited without naming
/// a number — so 1 s is *our* choice rather than the venue's, picked for the
/// same reason: an unpublished limit is cause for more caution, not less.
///
/// It does not bind today, and the payload says why. One request prices every
/// currency, and the response carries the instant of the next refresh
/// ([`ErApiSnapshot::next_update`]), so a caller polling faster than daily is
/// re-reading a value the provider has already told it will not change. The
/// pacing lever that matters is the caller's poll interval; this floor is only
/// the backstop.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

/// One batched er-api reading: the inverted rates, plus the provider's own
/// timestamps for the snapshot.
///
/// The timestamps are the reason this venue does not ride a bare
/// [`Quotes`] map the way [`frankfurter`](super::frankfurter) does, and they are
/// not decoration. This is a **daily** snapshot refreshed near 00:00 UTC, so the
/// instant a reading was fetched is not the instant it describes — often by many
/// hours. A consumer that stamps these at fetch time records a stale value as
/// fresh, which is the precise failure the roster's mixed publication
/// conventions make easy: this source, a mid-European-session reference fix, and
/// a streaming tape do not agree about what "now" means, and only the first of
/// those hands you its own answer.
#[derive(Clone, Debug, PartialEq)]
pub struct ErApiSnapshot {
    /// Currency code → USD per unit of that currency.
    pub rates: Quotes<String>,
    /// Epoch seconds at which the provider last refreshed these rates — the
    /// true instant of the observation, and what a store should key on.
    pub last_update: i64,
    /// Epoch seconds at which the provider says the next refresh lands. A
    /// caller polling before this is guaranteed the same values.
    pub next_update: i64,
}

/// A poll [`Source`] over er-api's batched latest-rates endpoint, keyed by ISO
/// currency code.
pub struct ErApiSource {
    http: HttpClient,
    currencies: Vec<String>,
}

impl ErApiSource {
    /// Build the source over `base_url`, keeping `currencies` from every poll.
    ///
    /// The endpoint is keyed by base currency in its **path** and takes no query
    /// parameters, so it always returns the provider's full table; `currencies`
    /// filters that table rather than narrowing the request. Passing fewer
    /// currencies therefore saves no bandwidth — it only keeps the roster's
    /// shape decided by the caller, as every batched venue here does.
    pub fn new(base_url: &str, currencies: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            currencies,
        })
    }

    /// Fetch every currency this source was built with, in one request.
    /// Currencies the provider does not carry are **omitted** rather than
    /// erroring, per the batched-poll convention in [`venues`](super).
    pub async fn poll(&self) -> Result<ErApiSnapshot> {
        let body: Value = self.http.get_json("/v6/latest/USD", &[]).await?;
        let currencies: Vec<&str> = self.currencies.iter().map(String::as_str).collect();
        parse_erapi(&body, &currencies)
    }
}

/// This source's [`Source::name`] — see [`crate::venues::pyth::FEED_NAME`] for
/// why the name is a constant rather than a literal at each use.
pub const FEED_NAME: &str = "erapi";

#[async_trait]
impl Source for ErApiSource {
    type Record = ErApiSnapshot;
    fn name(&self) -> &str {
        FEED_NAME
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode er-api's `{"result":"success","rates":{"<ccy>":<rate>}}` response —
/// `<ccy>` per USD — inverting each into USD per `<ccy>` and keeping only
/// positive finite rates.
///
/// **A non-`success` body is an error, not an empty reading.** This venue
/// answers a rejected or malformed request with HTTP 200 carrying
/// `{"result":"error"}`, so treating the absence of rates as "no currencies
/// quoted" would report a broken feed as a healthy one covering nothing. That
/// distinction — a response received is not a rate quoted — is what makes a
/// coverage gap visible instead of silent, so it is drawn here rather than left
/// to the caller.
pub fn parse_erapi(body: &Value, currencies: &[&str]) -> Result<ErApiSnapshot> {
    let result = body.get("result").and_then(Value::as_str).unwrap_or_default();
    if result != "success" {
        // The provider names the failure in `error-type`; carrying it through
        // is what makes a quota trip distinguishable from a malformed base.
        let detail = body
            .get("error-type")
            .and_then(Value::as_str)
            .unwrap_or("no error-type given");
        return Err(anyhow!("er-api returned result `{result}`: {detail}"));
    }
    let mut rates = Quotes::new();
    if let Some(table) = body.get("rates") {
        for &ccy in currencies {
            if let Some(rate) = table.get(ccy).and_then(Value::as_f64) {
                if rate.is_finite() && rate > 0.0 {
                    rates.insert(ccy.to_string(), 1.0 / rate);
                }
            }
        }
    }
    Ok(ErApiSnapshot {
        rates,
        last_update: body
            .get("time_last_update_unix")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        next_update: body
            .get("time_next_update_unix")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn success_body() -> Value {
        json!({
            "result": "success",
            "time_last_update_unix": 1_787_529_751_i64,
            "time_next_update_unix": 1_787_618_061_i64,
            "base_code": "USD",
            "rates": { "EUR": 0.85618, "MYR": 4.04086, "NGN": 1345.213025 }
        })
    }

    #[test]
    fn the_erapi_floor_is_stricter_than_the_shared_default() {
        // This venue publishes no rate limit, so there is no documented number
        // to check against and no venue arithmetic to assert. What can be
        // checked is the claim the constant actually makes: that the floor was
        // deliberately raised rather than left to inherit the shared default.
        assert!(MIN_REQUEST_INTERVAL > crate::http::DEFAULT_MIN_INTERVAL);
    }

    #[test]
    fn parses_and_inverts_erapi() {
        let out = parse_erapi(&success_body(), &["EUR", "MYR", "NGN"]).unwrap();
        // USD per EUR is the inverse of EUR per USD.
        assert!((out.rates["EUR"] - 1.0 / 0.85618).abs() < 1e-9);
        assert!((out.rates["MYR"] - 1.0 / 4.04086).abs() < 1e-9);
        assert!((out.rates["NGN"] - 1.0 / 1345.213025).abs() < 1e-12);
    }

    #[test]
    fn erapi_carries_the_providers_own_timestamps() {
        // The reason this venue does not ride a bare `Quotes`: a daily snapshot
        // stamped at fetch time records a hours-old value as current. Pinning
        // both instants is what lets a store key on the observation rather than
        // on the poll.
        let out = parse_erapi(&success_body(), &["EUR"]).unwrap();
        assert_eq!(out.last_update, 1_787_529_751);
        assert_eq!(out.next_update, 1_787_618_061);
    }

    #[test]
    fn erapi_omits_unquoted_currency() {
        let out = parse_erapi(&success_body(), &["EUR", "ZWL"]).unwrap();
        assert!(out.rates.contains_key("EUR"));
        assert!(!out.rates.contains_key("ZWL"));
    }

    #[test]
    fn an_error_result_is_an_error_not_an_empty_reading() {
        // The venue answers a rejected request with HTTP 200 and an error body,
        // so this is the difference between a feed reported broken and a feed
        // reported healthy while covering nothing.
        let body = json!({ "result": "error", "error-type": "invalid-key" });
        let err = parse_erapi(&body, &["EUR"]).expect_err("an error result must not parse");
        assert!(err.to_string().contains("invalid-key"), "{err}");
    }

    #[test]
    fn a_body_with_no_result_field_is_rejected() {
        // Absent rather than negative: a truncated or reshaped payload must not
        // read as success just because it carries no explicit failure.
        let body = json!({ "rates": { "EUR": 0.85618 } });
        assert!(parse_erapi(&body, &["EUR"]).is_err());
    }

    #[test]
    fn erapi_drops_a_nonsense_rate() {
        let body = json!({
            "result": "success",
            "rates": { "EUR": 0.85618, "AAA": 0.0, "BBB": -1.5 }
        });
        let out = parse_erapi(&body, &["EUR", "AAA", "BBB"]).unwrap();
        assert!(out.rates.contains_key("EUR"));
        // A zero would invert to infinity and a negative to a negative price;
        // both are dropped rather than propagated into the fair-value path.
        assert!(!out.rates.contains_key("AAA"));
        assert!(!out.rates.contains_key("BBB"));
    }
}
