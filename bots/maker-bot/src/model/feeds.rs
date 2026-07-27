//! Price-feed sources (§1).
//!
//! The price tiers as `feeds`-framework [`Source`]s over the shared async
//! [`HttpClient`]: each polls its REST endpoint on its own cadence and yields a
//! keyed batch the runner fans to a live forward sink (the tick loop drains it,
//! `tasks.rs`). Their readings feed the [`dropset_fair_value`] engine's
//! `fair = fx × basis` composition (mapped onto its legs in [`super::fair_mid`]);
//! they are not a primary-first cascade over one mid. By leg:
//!
//! - **ECB/Frankfurter** `/latest` — keyless `USD/<ccy>` inverted to a
//!   USD-per-unit rate: the **FX anchor** (the spec's designated anchor
//!   *fallback* tier; the streaming primaries below are a follow-up).
//! - **CoinGecko** `/simple/price` — one batched call prices every market's
//!   token in USD, plus `usd-coin` for the USDC/USD common-mode leg. This is
//!   the **crypto basis leg**, and it also supplies the anchor in the
//!   crypto-only (weekend / localnet) regime. It is **demoted** from the old
//!   cascade's primary mid — laggy and reflexive, never the FX anchor (§1).
//! - **CoinMarketCap** `/v2/cryptocurrency/quotes/latest` — batched by numeric
//!   id, keyed from `CMC_API_KEY` via the client's auth header; the basis-leg
//!   fallback when CoinGecko is stale (its ~10k/mo free quota rules out a hot
//!   poll). Wired only when the key is set ([`cmc_api_key`]).
//! - **Static** — a per-market constant ([`super::super::config::MarketConfig::static_usd`]),
//!   the last resort, supplied by the caller without a poll.
//!
//! The spec's streaming primaries — Pyth Hermes / OANDA for the anchor,
//! Coinbase `<token>/USDC` and Binance `EUR/USDT` for the basis, Circle
//! redemption for peg-truth — are a separate follow-up; until they land the
//! anchor runs on the Frankfurter fallback, so the two-peg model is live on
//! real data today.
//!
//! Each source's `next` yields one record — the latest batch keyed by the
//! identifier it was built with — which the supervisor caches with a read time
//! for the engine's freshness rules. The tiers no longer cascade in the
//! transport: each polls independently and the consumer's `legs()` picks the
//! freshest live leg (CoinGecko, else CoinMarketCap), so a stale tier simply
//! ages out rather than gating the next poll. The JSON shapes are decoded by
//! the free `parse_*` functions, unit tested against captured responses; only
//! the transport needs a network. Each source also exposes a one-shot `poll`
//! that backs the `--dry-run` credentials check.

use crate::config::CMC_KEY_ENV;
use anyhow::Result;
use async_trait::async_trait;
use dropset_feeds::{Batch, HttpClient, Source};
use serde_json::Value;
use std::collections::HashMap;

/// The CoinMarketCap API key for this run, or `None` when the secondary tier is
/// not wired up (the localnet demo runs without it). Read from the environment,
/// never a committed field.
pub fn cmc_api_key() -> Option<String> {
    std::env::var(CMC_KEY_ENV).ok().filter(|k| !k.is_empty())
}

/// CoinGecko `/simple/price` source: one batched USD quote for every id it was
/// built with (the crypto basis leg, plus `usd-coin` for the common-mode leg).
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

    /// Poll once: USD price for every id, omitting ids absent from the response.
    pub async fn poll(&self) -> Result<HashMap<String, f64>> {
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
    type Record = HashMap<String, f64>;
    fn name(&self) -> &str {
        "coingecko"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// CoinMarketCap `/v2/cryptocurrency/quotes/latest` source: USD quotes batched
/// by numeric id (the basis-leg fallback), authenticated with the API key.
pub struct CmcSource {
    http: HttpClient,
    ids: Vec<u32>,
}

impl CmcSource {
    /// Build the source over `base_url`, authenticating with `api_key` and
    /// batching `ids` in every poll.
    pub fn new(base_url: &str, ids: Vec<u32>, api_key: &str) -> Result<Self> {
        let http = HttpClient::new(base_url)?.with_header("X-CMC_PRO_API_KEY", api_key)?;
        Ok(Self { http, ids })
    }

    /// Poll once: USD price for every id, omitting ids absent from the response.
    pub async fn poll(&self) -> Result<HashMap<u32, f64>> {
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
    type Record = HashMap<u32, f64>;
    fn name(&self) -> &str {
        "coinmarketcap"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// ECB/Frankfurter `/latest?base=USD` source: the keyless FX anchor. The API
/// quotes `<ccy>` per USD; each reading is inverted to USD per `<ccy>`, the peg
/// a stablecoin tracks.
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

    /// Poll once: USD-per-unit peg for every currency, omitting those unquoted.
    pub async fn poll(&self) -> Result<HashMap<String, f64>> {
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
    type Record = HashMap<String, f64>;
    fn name(&self) -> &str {
        "frankfurter"
    }
    async fn next(&mut self) -> Result<Batch<Self::Record>> {
        Ok(Batch::new(vec![self.poll().await?]))
    }
}

/// Decode CoinGecko's `{"<id>":{"usd":<n>}}` batched simple-price response into
/// `id → usd`, keeping only positive finite readings.
pub fn parse_coingecko(body: &Value, ids: &[&str]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
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

/// Decode CoinMarketCap's `{"data":{"<id>":{"quote":{"USD":{"price":<n>}}}}}`
/// batched response into `id → usd`, keeping only positive finite readings.
pub fn parse_coinmarketcap(body: &Value, ids: &[u32]) -> HashMap<u32, f64> {
    let mut out = HashMap::new();
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

/// Decode Frankfurter's `{"rates":{"<ccy>":<rate>}}` response — `<ccy>` per USD
/// — and invert each into USD per `<ccy>`, the peg-rate proxy, keeping only
/// positive finite rates.
pub fn parse_frankfurter(body: &Value, currencies: &[&str]) -> HashMap<String, f64> {
    let mut out = HashMap::new();
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
