//! The ECB / Frankfurter `/latest` adapter — the keyless FX anchor, batched
//! across currencies.
//!
//! The API quotes `<ccy>` per USD; each reading is **inverted** to USD per
//! `<ccy>`, which is the peg a stablecoin tracks and the unit the fair-value
//! engine's anchor leg expects. It is the spec's designated anchor *fallback*
//! tier — daily ECB reference rates, not a streaming primary — so it carries
//! the anchor until Pyth Hermes / OANDA land (docs/data-feeds.md §9).

use super::Quotes;
use crate::{Batch, HttpClient, Source};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

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
            http: HttpClient::new(base_url)?,
            currencies,
        })
    }

    /// Fetch every currency this source was built with, in one request.
    /// Currencies the ECB set does not carry are **omitted** rather than
    /// erroring, per the batched-poll convention in [`venues`](super).
    pub async fn poll(&self) -> Result<Quotes<String>> {
        let csv = self.currencies.join(",");
        let body: Value = self
            .http
            .get_json("/latest", &[("base", "USD"), ("symbols", &csv)])
            .await?;
        let currencies: Vec<&str> = self.currencies.iter().map(String::as_str).collect();
        Ok(parse_frankfurter(&body, &currencies))
    }
}

#[async_trait]
impl Source for FrankfurterSource {
    type Record = Quotes<String>;
    fn name(&self) -> &str {
        "frankfurter"
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
}
