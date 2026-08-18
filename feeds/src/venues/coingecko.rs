//! The CoinGecko `/simple/price` adapter — batched token/USD quotes.
//!
//! One request prices every id the source was built with, which is what lets a
//! whole roster ride a keyless tier's IP budget (docs/data-feeds.md §10). For
//! the maker this is the **crypto basis leg**, and the anchor in the
//! crypto-only (weekend / localnet) regime; it is never the FX anchor — laggy
//! and reflexive against the venues it prices (market-making.md §1).

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

/// A poll [`Source`] over CoinGecko's batched simple-price endpoint, keyed by
/// CoinGecko's own id slugs (`euro-coin`, `usd-coin`, …).
pub struct CoinGeckoSource {
    http: HttpClient,
    ids: Vec<String>,
}

impl CoinGeckoSource {
    /// Build the source over `base_url`, batching `ids` in every poll.
    pub fn new(base_url: &str, ids: Vec<String>) -> Result<Self> {
        Ok(Self {
            http: HttpClient::new(base_url)?,
            ids,
        })
    }

    /// Fetch every id this source was built with, in one request. Ids CoinGecko
    /// does not list are **omitted** rather than erroring, per the batched-poll
    /// convention in [`super`].
    pub async fn poll(&self) -> Result<Quotes<String>> {
        let csv = self.ids.join(",");
        let body: Value = self
            .http
            .get_json("/simple/price", &[("ids", &csv), ("vs_currencies", "usd")])
            .await?;
        let ids: Vec<&str> = self.ids.iter().map(String::as_str).collect();
        Ok(parse_coingecko(&body, &ids))
    }
}

#[async_trait]
impl Source for CoinGeckoSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        "coingecko"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode CoinGecko's `{"<id>":{"usd":<n>}}` batched simple-price response into
/// `id → usd`, keeping only positive finite readings.
pub fn parse_coingecko(body: &Value, ids: &[&str]) -> Quotes<String> {
    let mut out = Quotes::new();
    for &id in ids {
        if let Some(v) = body
            .get(id)
            .and_then(|v| v.get("usd"))
            .and_then(Value::as_f64)
        {
            if v.is_finite() && v > 0.0 {
                out.insert(id.to_string(), v);
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
    fn parses_coingecko_batch() {
        let body = json!({
            "euro-coin": { "usd": 1.141 },
            "idrx": { "usd": 0.000056 },
            "real-mxn": { "usd": 0.0573 }
        });
        let out = parse_coingecko(&body, &["euro-coin", "idrx", "real-mxn"]);
        assert_eq!(out["euro-coin"], 1.141);
        assert_eq!(out["idrx"], 0.000056);
        assert_eq!(out["real-mxn"], 0.0573);
    }

    #[test]
    fn coingecko_omits_missing_and_non_positive() {
        let body = json!({ "euro-coin": { "usd": 1.14 }, "xsgd": { "usd": 0.0 } });
        let out = parse_coingecko(&body, &["euro-coin", "xsgd", "tokenised-gbp"]);
        assert_eq!(out.len(), 1);
        assert!(out.contains_key("euro-coin"));
        // Zero price and an absent id are both dropped.
        assert!(!out.contains_key("xsgd"));
        assert!(!out.contains_key("tokenised-gbp"));
    }
}
