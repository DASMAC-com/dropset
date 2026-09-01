//! The CoinMarketCap **keyless public** `/public-api/v1/simple/price` adapter — batched
//! token/USD quotes by numeric id, the basis-leg fallback when CoinGecko is
//! stale.
//!
//! **This adapter is deliberately keyless, and that is a budget decision as
//! much as a convenience one.** CoinMarketCap's keyed free (`Basic`) plan
//! carries a **15,000 call-credit monthly quota**, and its plan table bills it
//! for *personal* use, so it fits neither a standing collector nor a commercial
//! deployment: at a 60 s cadence one poller spends ~43,800 credits a month,
//! nearly 3× the budget, and the per-market demo shape multiplies that again.
//! The keyless public route (`/public-api/…`) publishes **no monthly quota** —
//! the documented contract is per-IP rate pooling, answered with a 429. Rate is
//! the kind of constraint a floor can hold at all, where a monthly quota is not
//! (see `MIN_REQUEST_INTERVAL`), so taking the route that prices access as a
//! rate is what makes the cadence question go away rather than merely managed.
//!
//! **Two limits of that reasoning, since it is load-bearing.** First, the
//! keyless route still returns a `credit_count` per response, so it is *metered*
//! even though no monthly allowance is published — "no published quota" is the
//! honest claim, not "no accounting". Second, what was checked is the keyed
//! plan's personal-use billing; the keyless route's licensing terms were **not**
//! confirmed, and provider terms commonly bind all access however it is
//! authenticated. So treat the licensing half as a reason to prefer this route,
//! not as a finding that it is unrestricted.
//!
//! **What that trade gives up, stated plainly:** the keyed plan's quota was
//! per-account and therefore isolated, while a *pooled* per-IP limit is by
//! definition shared — with the other maker processes on the host, and with
//! every unrelated tenant behind the same egress IP. So this tier can now be
//! throttled by traffic that is not ours, which the in-process floor cannot see
//! or prevent. The cascade bounds the damage (this is the *secondary* basis
//! tier, and a failed poll yields absence rather than a wrong price), and that
//! containment is the reason the trade is acceptable — not an absence of
//! downside.
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

/// The response-body cap for this venue, well below the shared 8 MiB default.
///
/// A legitimate answer here is a few hundred bytes — one `{"id":…,"price":…}`
/// record per requested id plus a small `status` object — so 8 MiB is six
/// orders of magnitude of headroom nothing needs. That matters more on an
/// **unauthenticated** route than on a keyed one: there is no account gating who
/// can be induced to answer, and 8 MiB of minimal JSON records would parse into
/// a transient `Vec<Value>` orders of magnitude larger in the maker's own
/// process, on every poll. 64 KiB is ~100× the largest plausible response.
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

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
        let http = HttpClient::new(base_url)?
            .with_min_interval(MIN_REQUEST_INTERVAL)
            .with_max_response_bytes(MAX_RESPONSE_BYTES);
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

/// This source's [`Source::name`] — see [`crate::venues::pyth::FEED_NAME`] for
/// why the name is a constant rather than a literal at each use.
pub const FEED_NAME: &str = "coinmarketcap";

#[async_trait]
impl Source for CmcSource {
    type Record = Quotes<u32>;
    fn name(&self) -> &str {
        FEED_NAME
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
///
/// If the same id appears twice, the **last** record wins. Nothing in the
/// venue's contract forbids a duplicate and no reading of it is more correct
/// than another, so this is documented rather than defended against: an
/// arbitrary-but-stated choice beats an unstated one.
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
    fn the_coinmarketcap_floor_is_stricter_than_the_shared_default() {
        // The keyless route publishes no numeric rate, so as with Frankfurter
        // the checkable claim is that the floor was deliberately raised rather
        // than inherited — compared against the default itself, not against a
        // restatement of this constant's own literal.
        assert!(MIN_REQUEST_INTERVAL > crate::http::DEFAULT_MIN_INTERVAL);
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

    /// The shape the keyless endpoint actually answers with: `data` is an array
    /// of records, each carrying its own id, and the price is a bare number
    /// rather than a nested USD quote.
    ///
    /// The **envelope** is copied from a live response
    /// (`/public-api/v1/simple/price` on the configured host, which answered 200
    /// with this structure); the **ids and prices** are this module's own
    /// long-standing test values, substituted so the assertions below stay
    /// comparable with the rest of the crate's fixtures. Recorded precisely
    /// because "captured" and "shaped like a capture" are different claims, and
    /// only the first would justify treating the field types as confirmed.
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
    fn a_duplicated_id_resolves_to_the_last_record() {
        // Pins the documented tie-break so it cannot drift silently.
        let body = json!({
            "data": [
                { "id": 20641, "price": 1.1000 },
                { "id": 20641, "price": 1.1407 }
            ]
        });
        let out = parse_coinmarketcap(&body, &[20641]);
        assert!((out[&20641] - 1.1407).abs() < 1e-9);
    }

    #[test]
    fn a_record_whose_id_is_unusable_is_skipped_without_losing_the_others() {
        // The `else { continue }` arm: a missing, non-numeric, or out-of-range
        // id must drop only its own record.
        let body = json!({
            "data": [
                { "price": 9.9 },
                { "id": "20641", "price": 9.9 },
                { "id": 4294967296u64, "price": 9.9 },
                { "id": 8489, "price": 0.7705 }
            ]
        });
        let out = parse_coinmarketcap(&body, &[20641, 8489]);
        assert_eq!(out.len(), 1);
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
