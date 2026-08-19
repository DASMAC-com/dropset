//! The CoinMarketCap **keyless public** `/v1/simple/price` adapter — batched
//! token/USD quotes by numeric id, the basis-leg fallback when CoinGecko is
//! stale.
//!
//! **This adapter is deliberately keyless, and that is a budget decision as
//! much as a convenience one.** CoinMarketCap's keyed free (`Basic`) plan
//! carries a **15,000 call-credit monthly quota** and is licensed for
//! *personal* use, so it fits neither a standing collector nor a commercial
//! deployment: at a 60 s cadence one poller spends ~43,800 credits a month,
//! nearly 3× the budget, and the per-market demo shape multiplies that again.
//! The keyless public route (`/public-api/…`) has **no monthly quota** — its
//! limits are per-IP rate pooling, answered with a 429 — which is a constraint
//! this crate's shared client already handles. Rate is a thing a floor can
//! hold; a monthly quota is not (see [`MIN_REQUEST_INTERVAL`]), so choosing the
//! route without a quota is what makes the cadence question go away rather than
//! merely get managed.
//!
//! **Why `/v1/simple/price` rather than the keyless `/v3/cryptocurrency/quotes/
//! latest`.** Both are in the keyless subset and both cost one credit, but this
//! source needs exactly one number per id. `simple/price` answers with just
//! that — `{"data":[{"id":…,"price":…}]}` — where `quotes/latest` returns the
//! full listing record (tags, supply, market caps, a dozen change windows) and
//! runs kilobytes per id for the one field we read.
//!
//! Note the keyless response shape differs from the keyed API's in a way that
//! matters to the decoder: `data` is an **array of records**, not an object
//! keyed by id string.

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;

/// The keyless public path for batched spot prices by numeric id.
const SIMPLE_PRICE_PATH: &str = "/public-api/v1/simple/price";

/// The canonical name this venue's credential *would* carry
/// ([`crate::secrets`]), declared with the adapter like every other venue's.
///
/// **Nothing resolves it today.** This adapter is on the keyless public route
/// (see the module note), so it sends no credential at all and the secrets
/// provider is never asked for this name. It is kept rather than deleted because
/// the keyless route's *licensing* is unconfirmed — only the keyed plan's
/// personal-use billing was checked — so a commercial deployment may yet need
/// the keyed path, and this is the name it would resolve. Treat it as a
/// reserved declaration, not as evidence that a key is in use.
pub const SECRET_NAME: &str = "coinmarketcap/api-key";

/// The floor between two requests on this venue.
///
/// The keyless route publishes **no numeric rate limit** — the documented
/// contract is per-IP pooling and "back off on a 429" — so, as with
/// Frankfurter, this number is ours rather than the venue's. 2 s (30 a minute)
/// is deliberately well inside anything a public pooled endpoint is likely to
/// allow, because the pool is shared with every other caller on the egress IP
/// *and* the localnet demo runs one maker process per market.
///
/// **A minimum interval bounds a rate, never a quota.** It is in-process state:
/// it paces requests while the process is up and resets when it restarts, so it
/// says nothing about a day's or a month's total. That is precisely why this
/// adapter is on the keyless route, where there is no quota for it to fail to
/// enforce — not a gap papered over, but one designed out.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_secs(2);

/// A poll [`Source`] over CoinMarketCap's keyless batched price endpoint, keyed
/// by its own numeric listing ids.
pub struct CmcSource {
    http: HttpClient,
    ids: Vec<u32>,
}

impl CmcSource {
    /// Build the source over `base_url`, batching `ids` in every poll. No
    /// credential: see the module note on why this route is keyless.
    pub fn new(base_url: &str, ids: Vec<u32>) -> Result<Self> {
        let http = HttpClient::new(base_url)?.with_min_interval(MIN_REQUEST_INTERVAL);
        Ok(Self { http, ids })
    }

    /// Fetch every id this source was built with, in one request. Ids
    /// CoinMarketCap does not list are **omitted** rather than erroring, per the
    /// batched-poll convention in [`venues`](super).
    pub async fn poll(&self) -> Result<Quotes<u32>> {
        let csv = self
            .ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let body: Value = self
            .http
            .get_json(SIMPLE_PRICE_PATH, &[("ids", &csv)])
            .await?;
        Ok(parse_coinmarketcap(&body, &self.ids))
    }
}

#[async_trait]
impl Source for CmcSource {
    type Record = Quotes<u32>;
    fn name(&self) -> &str {
        "coinmarketcap"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode the keyless `{"data":[{"id":<n>,"price":<n>}]}` batched response into
/// `id → usd`, keeping only positive finite readings.
///
/// `data` is an **array**, so this indexes it by the record's own `id` field
/// rather than by position: the venue is under no obligation to answer in the
/// order asked, or to answer for every id at all, and a positional read would
/// silently mis-attribute a price to the wrong token if it ever reordered.
/// `ids` therefore selects what the caller cares about, and anything the
/// response omits is simply absent — the same contract every other batched
/// venue in this module has.
pub fn parse_coinmarketcap(body: &Value, ids: &[u32]) -> Quotes<u32> {
    let mut out = Quotes::new();
    let Some(records) = body.get("data").and_then(Value::as_array) else {
        return out;
    };
    for record in records {
        let Some(id) = record
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| u32::try_from(id).ok())
        else {
            continue;
        };
        if !ids.contains(&id) {
            continue;
        }
        if let Some(v) = record.get("price").and_then(Value::as_f64) {
            if v.is_finite() && v > 0.0 {
                out.insert(id, v);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_floor_is_stricter_than_the_shared_default() {
        // The keyless route publishes no numeric rate, so as with Frankfurter
        // the assertion is that the floor was deliberately raised rather than
        // inherited — the pool is shared across the egress IP and the demo runs
        // several maker processes.
        assert!(MIN_REQUEST_INTERVAL >= Duration::from_secs(2));
    }

    #[test]
    fn the_keyless_route_carries_no_credential() {
        // The whole reason this adapter is on `/public-api` is that the keyed
        // free plan prices access as a monthly quota (which a per-request floor
        // cannot hold) and licenses it for personal use only. A key reappearing
        // here would quietly reintroduce both, so the constructor takes none.
        let source = CmcSource::new("https://example.test", vec![20641]).unwrap();
        assert!(source.ids.contains(&20641));
        assert!(SIMPLE_PRICE_PATH.starts_with("/public-api/"));
    }

    /// The shape the keyless endpoint actually answers with, captured from
    /// `/public-api/v1/simple/price?ids=1,1027,3408` — `data` is an array of
    /// records, each carrying its own id, and the price is a bare number rather
    /// than a nested USD quote.
    fn captured_response() -> Value {
        json!({
            "data": [
                { "id": 20641, "price": 1.1407 },
                { "id": 8489, "price": 0.7705 }
            ],
            "status": {
                "timestamp": "2026-08-19T20:22:58.940Z",
                "error_code": "0",
                "error_message": "",
                "elapsed": 5,
                "credit_count": 1
            }
        })
    }

    #[test]
    fn parses_coinmarketcap_batch_by_id() {
        let out = parse_coinmarketcap(&captured_response(), &[20641, 8489]);
        assert!((out[&20641] - 1.1407).abs() < 1e-9);
        assert!((out[&8489] - 0.7705).abs() < 1e-9);
    }

    #[test]
    fn records_are_matched_by_their_own_id_not_by_position() {
        // The venue may answer in any order, and the decoder must not assume the
        // request's. Reversing the records must not swap the two prices.
        let body = json!({
            "data": [
                { "id": 8489, "price": 0.7705 },
                { "id": 20641, "price": 1.1407 }
            ]
        });
        let out = parse_coinmarketcap(&body, &[20641, 8489]);
        assert!((out[&20641] - 1.1407).abs() < 1e-9);
        assert!((out[&8489] - 0.7705).abs() < 1e-9);
    }

    #[test]
    fn an_id_the_caller_did_not_ask_for_is_ignored() {
        let body = json!({ "data": [{ "id": 1027, "price": 2112.15 }] });
        assert!(parse_coinmarketcap(&body, &[20641]).is_empty());
    }

    #[test]
    fn coinmarketcap_missing_data_is_empty() {
        let body = json!({ "status": { "error_code": 1001 } });
        assert!(parse_coinmarketcap(&body, &[20641]).is_empty());
    }

    #[test]
    fn a_keyed_style_object_response_yields_nothing_rather_than_a_wrong_price() {
        // The keyed API answered with `data` as an object keyed by id string.
        // Should this adapter ever be pointed back at that route, the decode
        // must come back empty rather than silently mis-read a price.
        let body = json!({
            "data": { "20641": { "quote": { "USD": { "price": 1.1407 } } } }
        });
        assert!(parse_coinmarketcap(&body, &[20641]).is_empty());
    }

    #[test]
    fn a_non_positive_or_non_finite_price_is_omitted() {
        let body = json!({
            "data": [
                { "id": 20641, "price": 0.0 },
                { "id": 8489, "price": -1.5 }
            ]
        });
        assert!(parse_coinmarketcap(&body, &[20641, 8489]).is_empty());
    }
}
