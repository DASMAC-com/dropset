//! Price feeds (§1).
//!
//! A tiered USD price source behind one [`Feeds`] poller, cascaded
//! primary-first by [`super::fair_mid`]:
//!
//! 1. **CoinGecko** `/simple/price` — one batched call prices every market's
//!    token in USD (the primary market feed).
//! 2. **CoinMarketCap** `/v2/cryptocurrency/quotes/latest` — batched by numeric
//!    id, keyed from `CMC_API_KEY`. The free tier's ~10k/mo quota rules out a
//!    hot poll, so it is the secondary, polled only when CoinGecko is down.
//! 3. **ECB/Frankfurter** `/latest` — the keyless FX-rate tier: `USD/<ccy>`
//!    inverted to a USD-per-unit peg, a pure peg rate (not a market price).
//! 4. **Static** — a per-market constant ([`super::super::config::MarketConfig::static_usd`]),
//!    the last resort, supplied by the caller without a poll.
//!
//! Each `poll_*` returns the latest batch keyed by the identifier the caller
//! asked for; the caller stamps the read time for the freshness rules in
//! [`super::fair_mid`]. The JSON shapes are decoded by the free `parse_*`
//! functions, unit tested against captured responses; only the transport needs
//! a network.

use crate::config::{FeedConfig, CMC_KEY_ENV};
use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// Which feed tier produced a reading — surfaced in logs and the dry run so an
/// operator can see which source is live for each market.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedTier {
    /// CoinGecko market price (primary).
    CoinGecko,
    /// CoinMarketCap market price (secondary).
    CoinMarketCap,
    /// ECB/Frankfurter FX peg rate (tertiary).
    FxRate,
    /// Static configured peg (last resort).
    Static,
}

impl FeedTier {
    /// A short label for logs.
    pub fn label(self) -> &'static str {
        match self {
            FeedTier::CoinGecko => "coingecko",
            FeedTier::CoinMarketCap => "coinmarketcap",
            FeedTier::FxRate => "fx-rate",
            FeedTier::Static => "static",
        }
    }

    /// Whether this tier is a real market price (not a peg-rate fallback). A
    /// market tier quotes healthy; the FX-rate and static tiers run degraded.
    pub fn is_market_price(self) -> bool {
        matches!(self, FeedTier::CoinGecko | FeedTier::CoinMarketCap)
    }
}

/// Blocking poller over the tiered feeds.
pub struct Feeds {
    agent: ureq::Agent,
    cfg: FeedConfig,
    cmc_key: Option<String>,
}

impl Feeds {
    /// Build a poller, reading the CoinMarketCap key from the environment if
    /// present.
    pub fn new(cfg: FeedConfig) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(5))
            .build();
        let cmc_key = std::env::var(CMC_KEY_ENV).ok().filter(|k| !k.is_empty());
        Self {
            agent,
            cfg,
            cmc_key,
        }
    }

    /// Whether the CoinMarketCap secondary tier is wired up this run (key set).
    pub fn coinmarketcap_enabled(&self) -> bool {
        self.cmc_key.is_some()
    }

    /// CoinGecko USD price for every `id`, in one batched `/simple/price` call
    /// (primary tier). Ids absent from the response are omitted from the map.
    pub fn poll_coingecko(&self, ids: &[&str]) -> Result<HashMap<String, f64>> {
        let url = format!("{}/simple/price", self.cfg.coingecko_base_url);
        let body: Value = self
            .agent
            .get(&url)
            .query("ids", &ids.join(","))
            .query("vs_currencies", "usd")
            .call()
            .context("coingecko request")?
            .into_json()
            .context("coingecko json")?;
        Ok(parse_coingecko(&body, ids))
    }

    /// CoinMarketCap USD price for every numeric `id`, batched by id (secondary
    /// tier). Requires the API key; ids absent from the response are omitted.
    pub fn poll_coinmarketcap(&self, ids: &[u32]) -> Result<HashMap<u32, f64>> {
        let key = self
            .cmc_key
            .as_ref()
            .ok_or_else(|| anyhow!("{CMC_KEY_ENV} not set"))?;
        let csv = ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let url = format!(
            "{}/v2/cryptocurrency/quotes/latest",
            self.cfg.coinmarketcap_base_url
        );
        let body: Value = self
            .agent
            .get(&url)
            .set("X-CMC_PRO_API_KEY", key)
            .query("id", &csv)
            .call()
            .context("coinmarketcap request")?
            .into_json()
            .context("coinmarketcap json")?;
        Ok(parse_coinmarketcap(&body, ids))
    }

    /// ECB/Frankfurter USD-per-unit peg for every `currency`, batched in one
    /// `/latest?base=USD` call (tertiary tier). The response quotes `<ccy>` per
    /// USD; this inverts it to USD per `<ccy>`, the peg a stablecoin tracks.
    pub fn poll_frankfurter(&self, currencies: &[&str]) -> Result<HashMap<String, f64>> {
        let url = format!("{}/latest", self.cfg.frankfurter_base_url);
        let body: Value = self
            .agent
            .get(&url)
            .query("base", "USD")
            .query("symbols", &currencies.join(","))
            .call()
            .context("frankfurter request")?
            .into_json()
            .context("frankfurter json")?;
        Ok(parse_frankfurter(&body, currencies))
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

    #[test]
    fn tier_market_price_classification() {
        assert!(FeedTier::CoinGecko.is_market_price());
        assert!(FeedTier::CoinMarketCap.is_market_price());
        assert!(!FeedTier::FxRate.is_market_price());
        assert!(!FeedTier::Static.is_market_price());
    }
}
