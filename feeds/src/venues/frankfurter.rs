//! The ECB / Frankfurter `/latest` adapter — the keyless FX anchor, batched
//! across currencies.
//!
//! The API quotes `<ccy>` per USD; each reading is **inverted** to USD per
//! `<ccy>`, which is the peg a stablecoin tracks and the unit the fair-value
//! engine's anchor leg expects. It is the spec's designated anchor *fallback*
//! tier — daily ECB reference rates, not a streaming primary — so it carries
//! the anchor until Pyth Hermes / OANDA land (docs/data-feeds.md §9).

use super::Quotes;
use crate::time::parse_civil_utc;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// The floor between two requests on this venue.
///
/// Frankfurter **publishes no rate limit** — it is keyless with soft,
/// unpublished fair-use limits — one of two venues in the roster in that
/// position (keyless CoinMarketCap is the other). So unlike the floors derived
/// from a documented number, 1 s is *our* choice rather than the venue's —
/// picked because an unpublished limit is a reason for more caution, not less,
/// and because the data cannot justify faster: these are ECB reference rates
/// that change once a business day, so no consumer has a use for a tighter
/// floor.
///
/// It does not bind today: one request prices every currency and the maker polls
/// every 300 s. If this venue ever needs to be polled harder, self-hosting the
/// open-source service is the documented answer rather than leaning on the
/// public instance's goodwill.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(1);

/// A Frankfurter reading together with the ECB reference date it belongs to.
///
/// The bare [`Quotes`] map [`FrankfurterSource`] yields is what the maker's
/// fair-value cascade consumes, and it is deliberately unchanged. A *store*
/// needs more than the rates: these are daily reference rates, so the instant a
/// reading was fetched is not the instant it describes, and stamping at fetch
/// time would record a value up to a business day old — over a weekend,
/// longer — as fresh to the second. This type carries the missing half.
pub struct FrankfurterSnapshot {
    /// Currency code → USD per unit of that currency.
    pub rates: Quotes<String>,
    /// **Midnight UTC of the ECB reference date** these rates belong to, in
    /// epoch seconds — the true instant of the observation, and what a store
    /// should key on. `None` when the provider omitted the field or sent one
    /// that does not parse, which is a caller's cue to fall back rather than
    /// to attribute the reading to the epoch.
    ///
    /// Midnight rather than the nominal 16:00 CET fix, deliberately: it is
    /// conservative in the only direction that is safe — the reading can look
    /// staler than it is, never fresher — and it avoids encoding an ECB
    /// schedule constant and its DST dependency for no decision value.
    pub reference_date: Option<i64>,
}

/// A poll [`Source`] over Frankfurter's batched latest-rates endpoint, keyed by
/// ISO currency code.
pub struct FrankfurterSource {
    http: HttpClient,
    currencies: Vec<String>,
}

impl FrankfurterSource {
    /// Build the source over `base_url`, batching `currencies` in every poll.
    pub fn new(base_url: &str, currencies: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL),
            currencies,
        })
    }

    /// Fetch every currency this source was built with, in one request.
    /// Currencies the ECB set does not carry are **omitted** rather than
    /// erroring, per the batched-poll convention in [`venues`](super).
    pub async fn poll(&self) -> Result<Quotes<String>> {
        Ok(self.poll_snapshot().await?.rates)
    }

    /// The same request as [`poll`](Self::poll), keeping the reference date the
    /// response carries alongside the rates.
    ///
    /// Both methods issue one identical request; they differ only in how much
    /// of the response survives. A consumer that stores readings wants this
    /// one — see [`FrankfurterSnapshot`].
    pub async fn poll_snapshot(&self) -> Result<FrankfurterSnapshot> {
        let csv = self.currencies.join(",");
        let body: Value = self
            .http
            .get_json("/latest", &[("base", "USD"), ("symbols", &csv)])
            .await?;
        let currencies: Vec<&str> = self.currencies.iter().map(String::as_str).collect();
        Ok(parse_frankfurter_snapshot(&body, &currencies))
    }
}

/// A poll [`Source`] yielding [`FrankfurterSnapshot`] rather than a bare
/// [`Quotes`] map — the same venue, the same single request, for a consumer
/// that keys stored readings on the reference date.
///
/// A separate type rather than a change to [`FrankfurterSource`] because that
/// source's `Record` is what the maker's fair-value cascade receives over its
/// broadcast channel; moving it would ripple into the quoting path for a
/// benefit only the store path can use.
pub struct FrankfurterSnapshots(FrankfurterSource);

impl FrankfurterSnapshots {
    /// Build the source over `base_url`, batching `currencies` in every poll.
    pub fn new(base_url: &str, currencies: Vec<String>) -> Result<Self> {
        Ok(Self(FrankfurterSource::new(base_url, currencies)?))
    }
}

#[async_trait]
impl Source for FrankfurterSnapshots {
    type Record = FrankfurterSnapshot;
    fn name(&self) -> &str {
        FEED_NAME
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.0.poll_snapshot().await?]))
    }
}

/// This source's [`Source::name`] — see [`crate::venues::pyth::FEED_NAME`] for
/// why the name is a constant rather than a literal at each use.
pub const FEED_NAME: &str = "frankfurter";

#[async_trait]
impl Source for FrankfurterSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        FEED_NAME
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode Frankfurter's `{"rates":{"<ccy>":<rate>}}` response — `<ccy>` per USD
/// — and invert each into USD per `<ccy>`, the peg-rate proxy, keeping only
/// positive finite rates.
pub fn parse_frankfurter(body: &Value, currencies: &[&str]) -> Quotes<String> {
    let mut out = Quotes::new();
    let Some(rates) = body.get("rates") else {
        return out;
    };
    for &ccy in currencies {
        if let Some(rate) = rates.get(ccy).and_then(Value::as_f64) {
            if rate.is_finite() && rate > 0.0 {
                out.insert(ccy.to_string(), 1.0 / rate);
            }
        }
    }
    out
}

/// Decode the same response as [`parse_frankfurter`], additionally reading the
/// `date` field the endpoint returns (`{"date":"2026-09-08","rates":{…}}`) into
/// midnight UTC of that day.
///
/// The date is a **civil calendar date with no zone** — the day the ECB
/// reference fix belongs to, not a timestamp — so resolving it to midnight UTC
/// is an interpretation this function makes rather than a conversion it reads
/// off the wire. See [`FrankfurterSnapshot::reference_date`] for why midnight.
///
/// A missing or unparseable date yields `None` rather than an error: the rates
/// in such a response are still good, and a caller that can fall back to its
/// own clock should not lose the whole poll over a stamp.
pub fn parse_frankfurter_snapshot(body: &Value, currencies: &[&str]) -> FrankfurterSnapshot {
    FrankfurterSnapshot {
        rates: parse_frankfurter(body, currencies),
        reference_date: body
            .get("date")
            .and_then(Value::as_str)
            .and_then(|date| parse_civil_utc(date).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_frankfurter_floor_is_stricter_than_the_shared_default() {
        // This venue publishes no rate, so there is no documented number to
        // check against and no venue arithmetic to assert. What can be checked
        // is the claim the constant actually makes: that the floor was
        // deliberately raised rather than left to inherit the shared default.
        // Comparing against the default itself — rather than restating this
        // constant's own literal — is what makes the test fail if either side
        // moves.
        assert!(MIN_REQUEST_INTERVAL > crate::http::DEFAULT_MIN_INTERVAL);
    }

    #[test]
    fn parses_and_inverts_frankfurter() {
        let body = json!({
            "amount": 1.0,
            "base": "USD",
            "rates": { "EUR": 0.87765, "IDR": 17903.0, "MXN": 17.468 }
        });
        let out = parse_frankfurter(&body, &["EUR", "IDR", "MXN"]);
        // USD per EUR is the inverse of EUR per USD; ≈ the EURC spot.
        assert!((out["EUR"] - 1.0 / 0.87765).abs() < 1e-9);
        assert!((out["IDR"] - 1.0 / 17903.0).abs() < 1e-12);
        assert!((out["MXN"] - 1.0 / 17.468).abs() < 1e-9);
    }

    #[test]
    fn frankfurter_omits_unquoted_currency() {
        let body = json!({ "rates": { "EUR": 0.88 } });
        let out = parse_frankfurter(&body, &["EUR", "ZAR"]);
        assert!(out.contains_key("EUR"));
        assert!(!out.contains_key("ZAR"));
    }

    #[test]
    fn the_reference_date_resolves_to_midnight_utc() {
        // The shape the live endpoint returns, captured 2026-09-08.
        let body = json!({
            "amount": 1.0,
            "base": "USD",
            "date": "2026-09-08",
            "rates": { "AUD": 1.3861, "CAD": 1.3805, "EUR": 0.86103 }
        });
        let snap = parse_frankfurter_snapshot(&body, &["AUD", "CAD", "EUR"]);
        // Midnight, not the 16:00 CET fix: the stamp must be divisible by a
        // whole day. Asserting the remainder rather than the literal is what
        // makes this a test of the *rule* rather than of one date's arithmetic.
        let stamp = snap.reference_date.expect("a well-formed date parses");
        assert_eq!(stamp.rem_euclid(86_400), 0);
        assert_eq!(stamp, crate::time::civil_to_epoch_secs(2026, 9, 8, 0, 0, 0));
        // The rates ride along unchanged — this parse is the other one plus a
        // stamp, and a divergence between the two would be silent.
        assert_eq!(snap.rates, parse_frankfurter(&body, &["AUD", "CAD", "EUR"]));
    }

    #[test]
    fn a_missing_or_bogus_date_costs_the_stamp_not_the_rates() {
        // Absent entirely.
        let body = json!({ "rates": { "EUR": 0.88 } });
        let snap = parse_frankfurter_snapshot(&body, &["EUR"]);
        assert_eq!(snap.reference_date, None);
        // The reading itself must survive: a caller with its own clock can
        // still record this, and dropping it would lose a day's observation
        // over a field that is not the observation.
        assert!(snap.rates.contains_key("EUR"));
        // Present but not a date. `2026-13-08` is the case a permissive
        // field-splitting parse would otherwise wave through as month 13.
        for bad in ["", "not-a-date", "2026-13-08", "2026-09"] {
            let body = json!({ "date": bad, "rates": { "EUR": 0.88 } });
            let snap = parse_frankfurter_snapshot(&body, &["EUR"]);
            assert_eq!(snap.reference_date, None, "{bad:?} must not parse");
            assert!(snap.rates.contains_key("EUR"));
        }
    }
}
