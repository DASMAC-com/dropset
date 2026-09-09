//! The ECB / Frankfurter `/latest` adapter — the keyless FX anchor, batched
//! across currencies.
//!
//! The API quotes `<ccy>` per USD; each reading is **inverted** to USD per
//! `<ccy>`, which is the peg a stablecoin tracks and the unit the fair-value
//! engine's anchor leg expects. It is the spec's designated anchor *fallback*
//! tier — daily ECB reference rates, not a streaming primary
//! (docs/data-feeds.md §9).
//!
//! **A permanent fallback, not a stand-in.** This used to say it carried the
//! anchor "until Pyth Hermes / OANDA land"; both have, and the role did not
//! change — a once-a-business-day administered fix is breadth and corroboration
//! by nature, so it must never be a live-quote lead however many streaming
//! sources exist. OANDA in particular does not supersede it everywhere: that
//! venue's instrument list is direction-fixed and cannot serve CAD/USD at all.

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
/// These are daily reference rates, so the instant a reading was fetched is not
/// the instant it describes: stamping at fetch time records a value up to a
/// business day old — over a weekend, longer — as fresh to the second. This
/// type carries the missing half.
///
/// Both a *store* keying readings on the reference date and the *maker's*
/// fair-value cascade consume this, the latter to age the fix from publication
/// rather than receipt. The bare [`Quotes`] map [`FrankfurterSource`] yields
/// remains for callers that want the rates and no stamp — the one-shot dry-run
/// path is one — and that source stays free of the date parse deliberately.
#[derive(Clone, Debug, PartialEq)]
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
    ///
    /// **Deliberately does not parse the response's `date`.** This is the
    /// rates-only decode, for a caller that has no use for the reference date
    /// — the maker's one-shot dry-run is the live instance — so parsing one
    /// here would widen what a malformed upstream response can reach for no
    /// benefit to that caller. The date is parsed only by
    /// [`poll_snapshot`](Self::poll_snapshot).
    ///
    /// Note this is **no longer** the method the maker's quoting path drives:
    /// that path moved to the snapshot variant in order to age the fix from
    /// publication rather than receipt. The narrow decode is kept because it is
    /// still the honest shape for a caller that wants rates alone, not because
    /// the quoting path depends on it.
    pub async fn poll(&self) -> Result<Quotes<String>> {
        let body = self.fetch().await?;
        let currencies: Vec<&str> = self.currencies.iter().map(String::as_str).collect();
        Ok(parse_frankfurter(&body, &currencies))
    }

    /// The same request as [`poll`](Self::poll), keeping the reference date the
    /// response carries alongside the rates.
    ///
    /// Both methods issue one identical request; they differ only in how much
    /// of the response is decoded. A consumer that stores readings wants this
    /// one — see [`FrankfurterSnapshot`].
    pub async fn poll_snapshot(&self) -> Result<FrankfurterSnapshot> {
        let body = self.fetch().await?;
        let currencies: Vec<&str> = self.currencies.iter().map(String::as_str).collect();
        Ok(parse_frankfurter_snapshot(&body, &currencies))
    }

    /// The one request both polls issue, so the two cannot drift apart in what
    /// they ask the venue for — only in how much of the answer they decode.
    async fn fetch(&self) -> Result<Value> {
        let csv = self.currencies.join(",");
        self.http
            .get_json("/latest", &[("base", "USD"), ("symbols", &csv)])
            .await
    }
}

/// A poll [`Source`] yielding [`FrankfurterSnapshot`] rather than a bare
/// [`Quotes`] map — the same venue, the same single request, for a consumer
/// that needs the reference date: one that keys stored readings on it, or one
/// that ages the fix from publication rather than receipt.
///
/// A separate type rather than a change to [`FrankfurterSource`] so that each
/// `Record` shape stays available on its own. The maker's quoting path now
/// takes *this* one over its broadcast channel — the ripple that decision
/// implies was accepted deliberately, because ageing a daily fix from receipt
/// reports it as seconds old however long ago it was published.
pub struct FrankfurterSnapshotSource(FrankfurterSource);

impl FrankfurterSnapshotSource {
    /// Build the source over `base_url`, batching `currencies` in every poll.
    pub fn new(base_url: &str, currencies: Vec<String>) -> Result<Self> {
        Ok(Self(FrankfurterSource::new(base_url, currencies)?))
    }
}

#[async_trait]
impl Source for FrankfurterSnapshotSource {
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
            .and_then(|date| parse_civil_utc(date).ok())
            // Floored to the day, so the field's documented contract holds by
            // **construction** rather than by trusting the input's shape.
            // `parse_civil_utc` accepts an optional time component, so a
            // provider that started sending `2026-09-08 16:00:00` would
            // otherwise yield a non-midnight stamp and silently break the
            // invariant `reference_date` promises — a change no test could
            // catch, since only a bare date is ever fed in. `div_euclid`
            // rather than `/` so a pre-1970 date floors downward too.
            .map(|secs| secs.div_euclid(86_400) * 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{json_response, request_line, serve_once_capturing};
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
        // An INDEPENDENT literal, deliberately not `civil_to_epoch_secs(…)`:
        // comparing the parse against the same function the parse calls pins
        // the string-to-(y, m, d) decode but proves nothing about the epoch
        // arithmetic, since both sides move together. 1_788_825_600 is
        // 2026-09-08T00:00:00Z, cross-checked against an external clock.
        assert_eq!(stamp, 1_788_825_600);
        // The rates ride along unchanged.
        assert_eq!(snap.rates, parse_frankfurter(&body, &["AUD", "CAD", "EUR"]));
    }

    #[test]
    fn a_dated_response_carrying_a_time_still_floors_to_midnight() {
        // The provider sends a bare date today, so this pins the *contract*
        // rather than current behavior: `reference_date` promises midnight
        // UTC, and `parse_civil_utc` accepts an optional time component — so
        // without the floor a provider change would silently yield a
        // non-midnight stamp that no other test could see.
        let body = json!({ "date": "2026-09-08 16:00:00", "rates": { "EUR": 0.86103 } });
        let snap = parse_frankfurter_snapshot(&body, &["EUR"]);
        assert_eq!(snap.reference_date, Some(1_788_825_600));
    }

    /// The live response shape, captured 2026-09-08.
    fn live_body() -> Value {
        json!({
            "amount": 1.0,
            "base": "USD",
            "date": "2026-09-08",
            "rates": { "AUD": 1.3861, "CAD": 1.3805, "EUR": 0.86103 }
        })
    }

    fn currencies() -> Vec<String> {
        ["AUD", "CAD", "EUR"].map(str::to_string).to_vec()
    }

    #[tokio::test]
    async fn the_snapshot_source_yields_exactly_one_reading_over_http() {
        // This test used to be a comment saying it could not be written. The
        // gap was real: `next` wraps its reading in `Batch::new(vec![…])`, and
        // emptying that vector compiles, passes every parse test above, and
        // yields a collector that polls forever writing nothing — which this
        // repo has already shipped once for real. The only route to `next` is
        // HTTP, and the crate's one loopback stub was private to `http.rs`;
        // it now lives in `crate::testing`, which is what makes this reachable.
        let (port, _head) = serve_once_capturing(json_response(&live_body().to_string())).await;
        let mut source =
            FrankfurterSnapshotSource::new(&format!("http://127.0.0.1:{port}"), currencies())
                .unwrap();

        let batch = source.next().await.expect("the stub answers one poll");

        assert_eq!(batch.records.len(), 1);
        let snap = &batch.records[0];
        // Both halves survive the round trip: the rates, and the reference date
        // that is this source's entire reason for existing beside the bare one.
        let expected = parse_frankfurter_snapshot(&live_body(), &["AUD", "CAD", "EUR"]);
        assert_eq!(snap.rates, expected.rates);
        assert_eq!(snap.reference_date, Some(1_788_825_600));
    }

    #[tokio::test]
    async fn the_bare_source_yields_exactly_one_reading_over_http() {
        // The sibling gap, and not redundant with the one above: this is the
        // `Record` the maker's fair-value cascade receives, and it is a
        // separate `Source` impl with its own `Batch::new(vec![…])` to empty.
        let (port, _head) = serve_once_capturing(json_response(&live_body().to_string())).await;
        let mut source =
            FrankfurterSource::new(&format!("http://127.0.0.1:{port}"), currencies()).unwrap();

        let batch = source.next().await.expect("the stub answers one poll");

        assert_eq!(batch.records.len(), 1);
        assert_eq!(
            batch.records[0],
            parse_frankfurter(&live_body(), &["AUD", "CAD", "EUR"])
        );
    }

    #[tokio::test]
    async fn a_failed_status_fails_the_poll_rather_than_yielding_an_empty_batch() {
        // The sibling of er-api's error-path test, and not redundant with it:
        // both venues share `HttpClient`, but each `poll` propagates its own
        // `self.fetch().await?`, and swallowing that into an empty reading
        // would report a dead feed as a healthy one covering no currencies.
        let (port, _head) = serve_once_capturing(
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n".to_vec(),
        )
        .await;

        // `let … else` rather than `expect_err`, which would need `Batch` to be
        // `Debug`; widening a public type to phrase a test is the wrong trade.
        let Err(err) =
            FrankfurterSnapshotSource::new(&format!("http://127.0.0.1:{port}"), currencies())
                .unwrap()
                .next()
                .await
        else {
            panic!("a 503 must not read as a successful poll");
        };
        // The status phrase, not the bare number — see the er-api twin for why
        // the ephemeral port makes a bare-number assertion unsound.
        assert!(
            format!("{err:?}").contains("503 Service Unavailable"),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn both_polls_ask_the_venue_the_identical_question() {
        // `fetch` exists so the two polls cannot drift apart in what they
        // request — only in how much of the answer they decode. That promise is
        // invisible to any response-only assertion, so it needs the request
        // head: the two are compared against each other rather than against a
        // hand-written URL, which is what makes this a test of the *property*
        // rather than of today's query-string spelling.
        let (snapshot_port, snapshot_head) =
            serve_once_capturing(json_response(&live_body().to_string())).await;
        let (bare_port, bare_head) =
            serve_once_capturing(json_response(&live_body().to_string())).await;

        FrankfurterSource::new(&format!("http://127.0.0.1:{snapshot_port}"), currencies())
            .unwrap()
            .poll_snapshot()
            .await
            .expect("the stub answers one poll");
        FrankfurterSource::new(&format!("http://127.0.0.1:{bare_port}"), currencies())
            .unwrap()
            .poll()
            .await
            .expect("the stub answers one poll");

        let snapshot_head = snapshot_head.await.expect("a complete request head");
        let bare_head = bare_head.await.expect("a complete request head");
        assert_eq!(request_line(&snapshot_head), request_line(&bare_head));
        // And that shared request is the documented one: base-keyed by query,
        // unlike the sibling er-api venue's path-keyed endpoint.
        let line = request_line(&snapshot_head);
        assert!(line.starts_with("GET /latest?"), "{line}");
        assert!(line.contains("base=USD"), "{line}");
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
