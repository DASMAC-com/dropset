//! The CoinMarketCap `/v2/cryptocurrency/quotes/latest` adapter — batched
//! token/USD quotes by numeric id, the basis-leg fallback when CoinGecko is
//! stale. Its free tier's ~10k calls/month rules out a hot poll, so it is wired
//! on a slow cadence and only when a key is supplied.
//!
//! **The key is injected, not read here** — [`CmcSource::new`] takes it as an
//! argument and the caller decides where it came from (a process environment
//! today, a secrets provider later), so nothing in this adapter changes when
//! that answer does.

use super::{BatchQuotes, Quotes};
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// The header CoinMarketCap authenticates each request with.
const API_KEY_HEADER: &str = "X-CMC_PRO_API_KEY";

/// A poll [`Source`] over CoinMarketCap's batched quotes endpoint, keyed by its
/// own numeric listing ids.
pub struct CmcSource {
    http: HttpClient,
    ids: Vec<u32>,
}

impl CmcSource {
    /// Build the source over `base_url`, authenticating with `api_key` and
    /// batching `ids` in every poll.
    pub fn new(base_url: &str, ids: Vec<u32>, api_key: &str) -> Result<Self> {
        let http = HttpClient::new(base_url)?.with_header(API_KEY_HEADER, api_key)?;
        Ok(Self { http, ids })
    }
}

#[async_trait]
impl BatchQuotes for CmcSource {
    type Symbol = u32;

    async fn poll(&self) -> Result<Quotes<u32>> {
        let csv = self
            .ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let body: Value = self
            .http
            .get_json("/v2/cryptocurrency/quotes/latest", &[("id", &csv)])
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

/// Decode CoinMarketCap's `{"data":{"<id>":{"quote":{"USD":{"price":<n>}}}}}`
/// batched response into `id → usd`, keeping only positive finite readings.
pub fn parse_coinmarketcap(body: &Value, ids: &[u32]) -> Quotes<u32> {
    let mut out = Quotes::new();
    let Some(data) = body.get("data") else {
        return out;
    };
    for &id in ids {
        let price = data
            .get(id.to_string())
            .and_then(|d| d.get("quote"))
            .and_then(|q| q.get("USD"))
            .and_then(|u| u.get("price"))
            .and_then(Value::as_f64);
        if let Some(v) = price {
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
    fn parses_coinmarketcap_batch_by_id() {
        let body = json!({
            "data": {
                "20641": { "quote": { "USD": { "price": 1.1407 } } },
                "8489": { "quote": { "USD": { "price": 0.7705 } } }
            }
        });
        let out = parse_coinmarketcap(&body, &[20641, 8489]);
        assert!((out[&20641] - 1.1407).abs() < 1e-9);
        assert!((out[&8489] - 0.7705).abs() < 1e-9);
    }

    #[test]
    fn coinmarketcap_missing_data_is_empty() {
        let body = json!({ "status": { "error_code": 1001 } });
        assert!(parse_coinmarketcap(&body, &[20641]).is_empty());
    }

    #[test]
    fn an_injected_key_becomes_the_auth_header() {
        // The key reaches the transport as a header at build time, so a
        // malformed one fails here rather than on the first poll.
        assert!(CmcSource::new("https://example.test", vec![20641], "secret").is_ok());
        assert!(CmcSource::new("https://example.test", vec![20641], "bad\nvalue").is_err());
    }
}
